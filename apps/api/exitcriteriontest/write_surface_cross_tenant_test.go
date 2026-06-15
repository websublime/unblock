// write_surface_cross_tenant_test.go covers the round-16 / bead
// unblock-tv8.77 write-surface row-level tenant hardening THROUGH the MCP
// tool surface. Every item / milestone / deps write-by-id tool must reject
// a FOREIGN target id with NOT_FOUND and leave the foreign row untouched —
// the IDOR write seam the bead closes (SPEC §10.1.1).
//
// Threat model: a Bearer bound to org A (the f.OrgID fixture) names a ULID
// owned by a FOREIGN org B (seeded here directly, never via the caller's
// identity). The row-level tenant predicate keyed on the CallerOrgID
// internal channel (always pinned by the MCP handler from identity.OrgID)
// makes the foreign row invisible: the tool returns NOT_FOUND — the same
// envelope a genuinely missing id would yield — and the foreign DB row is
// asserted unchanged.
//
// Tools exercised (one cross-tenant case per hardened write tool):
//   - update            (workitems.Update — foreign item AND, per bead
//                         unblock-tv8.84, owned item + foreign milestone_id
//                         ⇒ NOT_FOUND, plus a same-org positive control)
//   - comment           (workitems.AppendComment, INSERT … SELECT gate)
//   - set_state         (workitems.SetStateColumns)
//   - close             (workitems.Close)
//   - claim             (workitems.Claim — NOT_FOUND, never ALREADY_CLAIMED)
//   - promote           (workitems.Promote)
//   - assign_item       (workitems.AssignItem — foreign item AND, post
//                         pre-QA cleanup, foreign milestone_id ⇒ NOT_FOUND
//                         not M-INV-7 PRECONDITION_NOT_MET)
//   - update_milestone  (workitems.UpdateMilestone)
//   - create_milestone  (workitems.CreateMilestone parent-read seam)
//   - add_dependency    (deps.AddEdge — foreign endpoint)
//   - remove_dependency (deps.RemoveEdge — foreign edge)
//
// The milestone_tree read seam already has a cross-tenant test
// (milestones_mcp_test.go::TestExitCriterion_MilestoneTree_CrossTenantRootRejected);
// this file is the WRITE-surface companion.
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 10.1.1 (write-surface row-level
// tenant gate) + § 4.4 / § 4.4.1 / § 4.5 (RPC CallerOrgID channel) + § 7
// (error envelope).

package exitcriteriontest_test

import (
	"context"
	"encoding/json"
	"testing"

	encoredb "encore.app/db"
)

// foreignTenant holds the directly-seeded foreign org/project + the rows a
// cross-tenant write would target. None of these are ever touched by the
// caller's identity — they exist only to be (un)reachable by id.
type foreignTenant struct {
	OrgID       string
	ProjectID   string
	UserID      string
	ItemID      string // a foreign work item (Backlog, is_ready=true → would be promotable IN its own org)
	ClaimedID   string // a foreign work item already claimed (would be closable IN its own org)
	MilestoneID string // a foreign org-scoped milestone (org_id set, project_id NULL)
	ProjMilesID string // a foreign project-scoped milestone (org_id NULL, project_id set) — locks the project_id branch of the milestone org-XOR-project gate
	EdgeID      string // a foreign edge ItemID → ClaimedID
	LabelID     string // a foreign org-scoped label (create-path label gate, bead unblock-tv8.78)
	CommentID   string // a foreign comment ON the foreign ItemID (AppendComment parent_id same-item gate, bead unblock-tv8.80)
}

// seedForeignTenant inserts a complete foreign org B (org, user, project,
// two items, a milestone, an edge) via direct sqldb writes — mirroring the
// labels_mcp_test.go / milestones_mcp_test.go cross-tenant seed pattern.
// ON DELETE CASCADE from org.organizations clears every child on cleanup.
func seedForeignTenant(t *testing.T, ctx context.Context) foreignTenant {
	t.Helper()
	ft := foreignTenant{
		OrgID:       mustULID(t, "foreign org"),
		ProjectID:   mustULID(t, "foreign project"),
		UserID:      mustULID(t, "foreign user"),
		ItemID:      mustULID(t, "foreign item"),
		ClaimedID:   mustULID(t, "foreign claimed item"),
		MilestoneID: mustULID(t, "foreign milestone"),
		ProjMilesID: mustULID(t, "foreign project milestone"),
		EdgeID:      mustULID(t, "foreign edge"),
		LabelID:     mustULID(t, "foreign label"),
		CommentID:   mustULID(t, "foreign comment"),
	}
	foreignSlug := "foreign-" + ft.OrgID[len(ft.OrgID)-8:]

	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO org.organizations (id, slug, name) VALUES ($1, $2, $3)`,
		ft.OrgID, foreignSlug, "Foreign Org B",
	); err != nil {
		t.Fatalf("insert foreign org: %v", err)
	}
	t.Cleanup(func() {
		// Background ctx because t.Context() is cancelled before cleanup
		// runs. ON DELETE CASCADE clears project/items/milestone/edge.
		_, _ = encoredb.DB.Exec(context.Background(), `DELETE FROM org.organizations WHERE id = $1`, ft.OrgID)
	})

	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO auth.users (id, primary_provider, primary_provider_id, email, display_name)
		 VALUES ($1, 'github', $2, $3, 'Foreign User')`,
		ft.UserID, "gh-"+ft.UserID[len(ft.UserID)-8:], "foreign-"+ft.UserID[len(ft.UserID)-8:]+"@example.com",
	); err != nil {
		t.Fatalf("insert foreign user: %v", err)
	}
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO org.projects (id, org_id, slug, name) VALUES ($1, $2, $3, $4)`,
		ft.ProjectID, ft.OrgID, "foreign-proj", "Foreign Project",
	); err != nil {
		t.Fatalf("insert foreign project: %v", err)
	}

	// A foreign Backlog+ready item (would be promotable / updatable in org B).
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO workitems.items (id, org_id, project_id, type, title, status, is_ready)
		 VALUES ($1, $2, $3, 'task', 'Foreign Item', 'Backlog', true)`,
		ft.ItemID, ft.OrgID, ft.ProjectID,
	); err != nil {
		t.Fatalf("insert foreign item: %v", err)
	}
	// A foreign already-claimed InProgress item (would be closable in org B).
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, type, title, status, claimed_by_id, claimed_at)
		 VALUES ($1, $2, $3, 'task', 'Foreign Claimed Item', 'InProgress', $4, now())`,
		ft.ClaimedID, ft.OrgID, ft.ProjectID, ft.UserID,
	); err != nil {
		t.Fatalf("insert foreign claimed item: %v", err)
	}
	// A foreign org-scoped milestone (org_id set, project_id NULL) — exercises
	// the org_id = $caller branch of the milestone org-XOR-project gate.
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO workitems.milestones (id, org_id, name, start_date, end_date)
		 VALUES ($1, $2, 'Foreign Milestone', '2026-01-01', '2026-12-31')`,
		ft.MilestoneID, ft.OrgID,
	); err != nil {
		t.Fatalf("insert foreign milestone: %v", err)
	}
	// A foreign project-scoped milestone (org_id NULL, project_id → org B's
	// project) — exercises the project_id IN (caller-org projects) branch of
	// the milestone org-XOR-project gate, the subtler tenant boundary the XOR
	// form exists for (bead unblock-tv8.78 pre-QA cleanup).
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO workitems.milestones (id, project_id, name, start_date, end_date)
		 VALUES ($1, $2, 'Foreign Project Milestone', '2026-01-01', '2026-12-31')`,
		ft.ProjMilesID, ft.ProjectID,
	); err != nil {
		t.Fatalf("insert foreign project milestone: %v", err)
	}
	// A foreign edge between the two foreign items.
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO deps.dependencies (id, from_item, to_item, kind, created_by)
		 VALUES ($1, $2, $3, 'blocks', $4)`,
		ft.EdgeID, ft.ItemID, ft.ClaimedID, ft.UserID,
	); err != nil {
		t.Fatalf("insert foreign edge: %v", err)
	}
	// A foreign org-scoped label (the create-path label gate target, bead
	// unblock-tv8.78). org-scoped → project_id NULL per the labels XOR check.
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO workitems.labels (id, org_id, name, color) VALUES ($1, $2, 'foreign-label', '#abcdef')`,
		ft.LabelID, ft.OrgID,
	); err != nil {
		t.Fatalf("insert foreign label: %v", err)
	}
	// A foreign comment ON the foreign item — the AppendComment parent_id
	// same-item gate target (bead unblock-tv8.80). A home caller threading a
	// reply whose parent_id is this id must get NOT_FOUND (foreign-org parent),
	// indistinguishable from a missing parent.
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO workitems.comments (id, item_id, author_id, kind, body)
		 VALUES ($1, $2, $3, 'general', 'Foreign comment')`,
		ft.CommentID, ft.ItemID, ft.UserID,
	); err != nil {
		t.Fatalf("insert foreign comment: %v", err)
	}
	return ft
}

// assertNotFound asserts the §7 error envelope carries kind == "NOT_FOUND".
func assertNotFound(t *testing.T, env jsonRPCEnvelope, tool string) {
	t.Helper()
	data := expectError(t, env)
	if data.Kind != "NOT_FOUND" {
		t.Fatalf("cross-tenant %s: kind = %q, want NOT_FOUND (foreign id must be invisible)", tool, data.Kind)
	}
}

// itemStatus reads the status column directly (the test goroutine has no
// Identity for an rbac.For read).
func itemStatus(t *testing.T, ctx context.Context, itemID string) string {
	t.Helper()
	var s string
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT status FROM workitems.items WHERE id = $1`, itemID,
	).Scan(&s); err != nil {
		t.Fatalf("read status for %s: %v", itemID, err)
	}
	return s
}

// TestExitCriterion_WriteSurface_CrossTenantRejected drives every hardened
// write tool with a FOREIGN target id under the fixture's Bearer and asserts
// NOT_FOUND + an untouched foreign row. SPEC §10.1.1.
func TestExitCriterion_WriteSurface_CrossTenantRejected(t *testing.T) {
	f := fx(t)
	sessionID := initializeSession(t, f.RawKey)
	ctx := t.Context()
	ft := seedForeignTenant(t, ctx)

	// --- update: foreign item ⇒ NOT_FOUND, title unchanged --------------
	t.Run("update", func(t *testing.T) {
		env := callTool(t, f.RawKey, sessionID, "update", map[string]any{
			"item_id": ft.ItemID,
			"title":   "hijacked title",
		})
		assertNotFound(t, env, "update")
		var title string
		if err := encoredb.DB.QueryRow(ctx,
			`SELECT title FROM workitems.items WHERE id = $1`, ft.ItemID,
		).Scan(&title); err != nil {
			t.Fatalf("read foreign title: %v", err)
		}
		if title != "Foreign Item" {
			t.Fatalf("cross-tenant update mutated foreign title to %q, want %q", title, "Foreign Item")
		}
	})

	// --- update: OWNED item, FOREIGN milestone_id ⇒ NOT_FOUND, owned
	//     milestone_id stays NULL -------------------------------------------
	// The caller owns itm_b; it passes a FOREIGN milestone_id. workitems.Update
	// writes the wire-supplied milestone_id into the OWNED item, so the
	// target-item gate ($7 = org_id) alone would let this through — the
	// existence-only FK passes for a foreign-but-existing milestone. The
	// milestone-write gate (bead unblock-tv8.84, §10.1.1) additionally requires
	// the milestone to belong to the caller's org (org-XOR-project predicate,
	// mirroring AssignItem / Create), so the foreign milestone is invisible:
	// the UPDATE matches zero rows → NOT_FOUND, indistinguishable from a missing
	// milestone, and the owned item's milestone_id stays NULL. Without the gate
	// this is the proven cross-tenant write IDOR.
	t.Run("update_foreign_milestone", func(t *testing.T) {
		ownedItem := f.ItemID("itm_b")
		env := callTool(t, f.RawKey, sessionID, "update", map[string]any{
			"item_id":      ownedItem,
			"milestone_id": ft.MilestoneID, // foreign org-scoped milestone — the reference under test
		})
		assertNotFound(t, env, "update (owned item, foreign milestone)")
		var milestoneID *string
		if err := encoredb.DB.QueryRow(ctx,
			`SELECT milestone_id FROM workitems.items WHERE id = $1`, ownedItem,
		).Scan(&milestoneID); err != nil {
			t.Fatalf("read owned item milestone_id: %v", err)
		}
		if milestoneID != nil {
			t.Fatalf("update with foreign milestone set owned milestone_id to %q, want NULL", *milestoneID)
		}
	})

	// --- update: OWNED item, FOREIGN PROJECT-scoped milestone_id ⇒ NOT_FOUND -
	// The org-scoped case above exercises the org_id = $caller branch of the
	// milestone org-XOR-project gate; this case (a foreign project-scoped
	// milestone, org_id NULL, project_id → org B's project) locks the OTHER
	// branch — project_id IN (caller-org projects) — so the full XOR predicate
	// on the update path is covered (bead unblock-tv8.84). The owned item's
	// milestone_id must stay NULL.
	t.Run("update_foreign_project_scoped_milestone", func(t *testing.T) {
		ownedItem := f.ItemID("itm_c")
		env := callTool(t, f.RawKey, sessionID, "update", map[string]any{
			"item_id":      ownedItem,
			"milestone_id": ft.ProjMilesID, // foreign project-scoped milestone — the reference under test
		})
		assertNotFound(t, env, "update (owned item, foreign project-scoped milestone)")
		var milestoneID *string
		if err := encoredb.DB.QueryRow(ctx,
			`SELECT milestone_id FROM workitems.items WHERE id = $1`, ownedItem,
		).Scan(&milestoneID); err != nil {
			t.Fatalf("read owned item milestone_id: %v", err)
		}
		if milestoneID != nil {
			t.Fatalf("update with foreign project-scoped milestone set owned milestone_id to %q, want NULL", *milestoneID)
		}
	})

	// --- update: OWNED item, SAME-ORG milestone_id ⇒ SUCCESS, assigned -------
	// The positive control: a home caller assigning a HOME org-scoped milestone
	// to its OWN item via update must SUCCEED and stamp the column. Proves the
	// milestone-write gate (bead unblock-tv8.84) does not over-reject legitimate
	// same-org milestone assignment through the update path.
	t.Run("update_same_org_milestone", func(t *testing.T) {
		ownedItem := f.ItemID("itm_d")
		homeMilestone := mustULID(t, "home milestone for update positive control")
		if _, err := encoredb.DB.Exec(ctx,
			`INSERT INTO workitems.milestones (id, org_id, name, start_date, end_date)
			 VALUES ($1, $2, 'Home Milestone', '2026-01-01', '2026-12-31')`,
			homeMilestone, f.OrgID,
		); err != nil {
			t.Fatalf("seed home milestone: %v", err)
		}
		t.Cleanup(func() {
			// Detach from the item first (the FK would otherwise block the delete)
			// then drop the milestone.
			_, _ = encoredb.DB.Exec(context.Background(), `UPDATE workitems.items SET milestone_id = NULL WHERE id = $1`, ownedItem)
			_, _ = encoredb.DB.Exec(context.Background(), `DELETE FROM workitems.milestones WHERE id = $1`, homeMilestone)
		})
		env := callTool(t, f.RawKey, sessionID, "update", map[string]any{
			"item_id":      ownedItem,
			"milestone_id": homeMilestone, // same-org milestone — must assign
		})
		var out struct {
			Item struct {
				ID          string `json:"id"`
				MilestoneID string `json:"milestone_id"`
			} `json:"item"`
		}
		if err := json.Unmarshal(expectSuccess(t, env), &out); err != nil {
			t.Fatalf("unmarshal update success: %v", err)
		}
		if out.Item.MilestoneID != homeMilestone {
			t.Fatalf("same-org update milestone_id = %q, want %q", out.Item.MilestoneID, homeMilestone)
		}
		var milestoneID *string
		if err := encoredb.DB.QueryRow(ctx,
			`SELECT milestone_id FROM workitems.items WHERE id = $1`, ownedItem,
		).Scan(&milestoneID); err != nil {
			t.Fatalf("read owned item milestone_id: %v", err)
		}
		if milestoneID == nil || *milestoneID != homeMilestone {
			t.Fatalf("same-org update persisted milestone_id %v, want %q", milestoneID, homeMilestone)
		}
	})

	// --- comment: foreign item ⇒ NOT_FOUND, no comment inserted ------
	// The fixture seeds one comment on the foreign item (ft.CommentID, the
	// parent_id gate target below); the cross-tenant call must add nothing, so
	// the count stays at its seeded baseline.
	t.Run("comment", func(t *testing.T) {
		before := countItemComments(t, ctx, ft.ItemID)
		env := callTool(t, f.RawKey, sessionID, "comment", map[string]any{
			"item_id": ft.ItemID,
			"body":    "intruder comment",
		})
		assertNotFound(t, env, "comment")
		if after := countItemComments(t, ctx, ft.ItemID); after != before {
			t.Fatalf("cross-tenant comment changed foreign item comment count %d → %d, want unchanged", before, after)
		}
	})

	// --- comment: foreign-org parent_id ⇒ NOT_FOUND, no reply inserted ---
	// Home caller, home item, but parent_id = a comment that lives in the
	// FOREIGN org (on the foreign item). The same-item parent_id gate (bead
	// unblock-tv8.80, §10.1.1 / §6.2 Tool 10) must reject this with NOT_FOUND
	// — indistinguishable from a missing parent — and insert zero rows.
	t.Run("comment_foreign_parent_id", func(t *testing.T) {
		homeItem := f.ItemID("itm_a")
		before := countItemComments(t, ctx, homeItem)
		env := callTool(t, f.RawKey, sessionID, "comment", map[string]any{
			"item_id":   homeItem,
			"parent_id": ft.CommentID, // foreign-org comment — the reference under test
			"body":      "reply under a foreign comment",
		})
		assertNotFound(t, env, "comment (foreign parent_id)")
		if after := countItemComments(t, ctx, homeItem); after != before {
			t.Fatalf("comment with foreign parent_id changed home item comment count %d → %d, want unchanged", before, after)
		}
	})

	// --- comment: cross-item (same-org) parent_id ⇒ NOT_FOUND ------------
	// Home caller, home item itm_b, parent_id = a comment that exists in the
	// SAME org but on a DIFFERENT home item (itm_a). The gate is same-item, not
	// merely same-org, so this must also be NOT_FOUND with zero rows inserted
	// (bead unblock-tv8.80 — cross-item threading is rejected even within a tenant).
	t.Run("comment_cross_item_parent_id", func(t *testing.T) {
		itemA := f.ItemID("itm_a")
		itemB := f.ItemID("itm_b")
		// Seed a home comment ON itm_a (same org as the caller).
		parentOnA := mustULID(t, "home parent on itm_a")
		if _, err := encoredb.DB.Exec(ctx,
			`INSERT INTO workitems.comments (id, item_id, author_id, kind, body)
			 VALUES ($1, $2, $3, 'general', 'home comment on itm_a')`,
			parentOnA, itemA, f.UserID,
		); err != nil {
			t.Fatalf("seed home comment on itm_a: %v", err)
		}
		t.Cleanup(func() {
			_, _ = encoredb.DB.Exec(context.Background(), `DELETE FROM workitems.comments WHERE id = $1`, parentOnA)
		})
		before := countItemComments(t, ctx, itemB)
		env := callTool(t, f.RawKey, sessionID, "comment", map[string]any{
			"item_id":   itemB,     // a DIFFERENT home item
			"parent_id": parentOnA, // same-org comment, but on itm_a — the reference under test
			"body":      "reply on itm_b pointing at an itm_a comment",
		})
		assertNotFound(t, env, "comment (cross-item parent_id)")
		if after := countItemComments(t, ctx, itemB); after != before {
			t.Fatalf("comment with cross-item parent_id changed itm_b comment count %d → %d, want unchanged", before, after)
		}
	})

	// --- comment: same-item parent_id ⇒ SUCCESS, reply threaded ----------
	// The positive control: a home caller threading a reply on the SAME home
	// item as the parent comment must SUCCEED and echo parent_id (proves the
	// same-item gate does not over-reject legitimate threading).
	t.Run("comment_same_item_parent_id", func(t *testing.T) {
		itemA := f.ItemID("itm_a")
		parentOnA := mustULID(t, "home parent for same-item thread")
		if _, err := encoredb.DB.Exec(ctx,
			`INSERT INTO workitems.comments (id, item_id, author_id, kind, body)
			 VALUES ($1, $2, $3, 'general', 'home parent for same-item thread')`,
			parentOnA, itemA, f.UserID,
		); err != nil {
			t.Fatalf("seed same-item parent comment: %v", err)
		}
		t.Cleanup(func() {
			_, _ = encoredb.DB.Exec(context.Background(), `DELETE FROM workitems.comments WHERE parent_id = $1 OR id = $1`, parentOnA)
		})
		env := callTool(t, f.RawKey, sessionID, "comment", map[string]any{
			"item_id":   itemA,
			"parent_id": parentOnA, // a comment on the SAME item — must thread
			"body":      "valid same-item reply",
		})
		var out struct {
			Comment struct {
				ID       string `json:"id"`
				ItemID   string `json:"item_id"`
				ParentID string `json:"parent_id"`
			} `json:"comment"`
		}
		if err := json.Unmarshal(expectSuccess(t, env), &out); err != nil {
			t.Fatalf("unmarshal comment success: %v", err)
		}
		if out.Comment.ParentID != parentOnA {
			t.Fatalf("same-item reply parent_id = %q, want %q", out.Comment.ParentID, parentOnA)
		}
		if out.Comment.ItemID != itemA {
			t.Fatalf("same-item reply item_id = %q, want %q", out.Comment.ItemID, itemA)
		}
	})

	// --- set_state: foreign item ⇒ NOT_FOUND, impl_state unchanged ------
	t.Run("set_state", func(t *testing.T) {
		env := callTool(t, f.RawKey, sessionID, "set_state", map[string]any{
			"item_id":    ft.ItemID,
			"impl_state": "done",
		})
		assertNotFound(t, env, "set_state")
		var impl string
		if err := encoredb.DB.QueryRow(ctx,
			`SELECT impl_state FROM workitems.items WHERE id = $1`, ft.ItemID,
		).Scan(&impl); err != nil {
			t.Fatalf("read foreign impl_state: %v", err)
		}
		if impl == "done" {
			t.Fatalf("cross-tenant set_state mutated foreign impl_state to %q", impl)
		}
	})

	// --- close: foreign CLAIMED item ⇒ NOT_FOUND (gate runs BEFORE AF3),
	//     status stays InProgress -----------------------------------------
	t.Run("close", func(t *testing.T) {
		env := callTool(t, f.RawKey, sessionID, "close", map[string]any{
			"item_id": ft.ClaimedID,
		})
		assertNotFound(t, env, "close")
		if got := itemStatus(t, ctx, ft.ClaimedID); got != "InProgress" {
			t.Fatalf("cross-tenant close mutated foreign claimed item status to %q, want InProgress", got)
		}
	})

	// --- claim: foreign item ⇒ NOT_FOUND (never ALREADY_CLAIMED), the
	//     foreign claimed item keeps its claimer -------------------------
	t.Run("claim", func(t *testing.T) {
		env := callTool(t, f.RawKey, sessionID, "claim", map[string]any{
			"item_id": ft.ClaimedID,
		})
		// The bead AC is explicit: a foreign claimed item must yield
		// NOT_FOUND, NOT the ALREADY_CLAIMED loser envelope — the tenant
		// gate fires before the §6.4 loser discrimination.
		assertNotFound(t, env, "claim")
		var claimedBy *string
		if err := encoredb.DB.QueryRow(ctx,
			`SELECT claimed_by_id FROM workitems.items WHERE id = $1`, ft.ClaimedID,
		).Scan(&claimedBy); err != nil {
			t.Fatalf("read foreign claimed_by_id: %v", err)
		}
		if claimedBy == nil || *claimedBy != ft.UserID {
			t.Fatalf("cross-tenant claim altered foreign claimed_by_id (got %v, want %q)", claimedBy, ft.UserID)
		}
	})

	// --- promote: foreign Backlog+ready item ⇒ NOT_FOUND, status stays
	//     Backlog ----------------------------------------------------------
	t.Run("promote", func(t *testing.T) {
		env := callTool(t, f.RawKey, sessionID, "promote", map[string]any{
			"item_id": ft.ItemID,
		})
		assertNotFound(t, env, "promote")
		if got := itemStatus(t, ctx, ft.ItemID); got != "Backlog" {
			t.Fatalf("cross-tenant promote mutated foreign item status to %q, want Backlog", got)
		}
	})

	// --- assign_item: foreign item ⇒ NOT_FOUND, milestone_id stays NULL --
	t.Run("assign_item", func(t *testing.T) {
		env := callTool(t, f.RawKey, sessionID, "assign_item", map[string]any{
			"item_id":      ft.ItemID,
			"milestone_id": ft.MilestoneID,
		})
		assertNotFound(t, env, "assign_item")
		var milestoneID *string
		if err := encoredb.DB.QueryRow(ctx,
			`SELECT milestone_id FROM workitems.items WHERE id = $1`, ft.ItemID,
		).Scan(&milestoneID); err != nil {
			t.Fatalf("read foreign item milestone_id: %v", err)
		}
		if milestoneID != nil {
			t.Fatalf("cross-tenant assign_item set foreign milestone_id to %q, want NULL", *milestoneID)
		}
	})

	// --- assign_item: OWNED item, FOREIGN milestone_id ⇒ NOT_FOUND --------
	// The caller owns itm_a; it passes a FOREIGN milestone_id. The milestone
	// read is now CallerOrgID-gated (bead unblock-tv8.77 pre-QA cleanup,
	// §10.1.1), so the foreign milestone is invisible and the tool returns
	// NOT_FOUND — it MUST NOT surface as M-INV-7 PRECONDITION_NOT_MET, which
	// would disclose the foreign milestone's existence/scope via error
	// discrimination. The owned item's milestone_id must stay NULL.
	t.Run("assign_item_foreign_milestone", func(t *testing.T) {
		ownedItem := f.ItemID("itm_a")
		env := callTool(t, f.RawKey, sessionID, "assign_item", map[string]any{
			"item_id":      ownedItem,
			"milestone_id": ft.MilestoneID,
		})
		assertNotFound(t, env, "assign_item (owned item, foreign milestone)")
		var milestoneID *string
		if err := encoredb.DB.QueryRow(ctx,
			`SELECT milestone_id FROM workitems.items WHERE id = $1`, ownedItem,
		).Scan(&milestoneID); err != nil {
			t.Fatalf("read owned item milestone_id: %v", err)
		}
		if milestoneID != nil {
			t.Fatalf("assign_item with foreign milestone set owned milestone_id to %q, want NULL", *milestoneID)
		}
	})

	// --- update_milestone: foreign milestone ⇒ NOT_FOUND, name unchanged -
	t.Run("update_milestone", func(t *testing.T) {
		env := callTool(t, f.RawKey, sessionID, "update_milestone", map[string]any{
			"milestone_id": ft.MilestoneID,
			"name":         "hijacked milestone",
		})
		assertNotFound(t, env, "update_milestone")
		var name string
		if err := encoredb.DB.QueryRow(ctx,
			`SELECT name FROM workitems.milestones WHERE id = $1`, ft.MilestoneID,
		).Scan(&name); err != nil {
			t.Fatalf("read foreign milestone name: %v", err)
		}
		if name != "Foreign Milestone" {
			t.Fatalf("cross-tenant update_milestone mutated foreign name to %q", name)
		}
	})

	// --- create_milestone with a FOREIGN parent_milestone_id ⇒ NOT_FOUND;
	//     the parent-read seam must not leak a foreign parent's scope/dates,
	//     and no child milestone is created under the caller's org --------
	t.Run("create_milestone_foreign_parent", func(t *testing.T) {
		env := callTool(t, f.RawKey, sessionID, "create_milestone", map[string]any{
			"parent_milestone_id": ft.MilestoneID,
			"name":                "intruder child",
			"start_date":          "2026-02-01",
			"end_date":            "2026-03-01",
		})
		assertNotFound(t, env, "create_milestone")
		var n int
		if err := encoredb.DB.QueryRow(ctx,
			`SELECT COUNT(*) FROM workitems.milestones
			  WHERE parent_milestone_id = $1 AND lower(name) = 'intruder child'`,
			ft.MilestoneID,
		).Scan(&n); err != nil {
			t.Fatalf("count intruder child milestones: %v", err)
		}
		if n != 0 {
			t.Fatalf("cross-tenant create_milestone created %d child milestone(s) under foreign parent, want 0", n)
		}
	})

	// --- add_dependency between FOREIGN items ⇒ NOT_FOUND, zero new edges -
	t.Run("add_dependency", func(t *testing.T) {
		var before int
		if err := encoredb.DB.QueryRow(ctx,
			`SELECT COUNT(*) FROM deps.dependencies WHERE from_item = $1 OR to_item = $1`, ft.ClaimedID,
		).Scan(&before); err != nil {
			t.Fatalf("count foreign edges before: %v", err)
		}
		// Add a (related) edge ClaimedID → ItemID in org B. The caller's
		// add_dependency resolves to_item first via workitems.Get (rbac.For),
		// which already yields NOT_FOUND for a foreign to_item — so this also
		// proves the handler's pre-resolution gate. Either way: NOT_FOUND.
		env := callTool(t, f.RawKey, sessionID, "add_dependency", map[string]any{
			"from_item_id": ft.ClaimedID,
			"to_item_id":   ft.ItemID,
			"kind":         "related",
		})
		assertNotFound(t, env, "add_dependency")
		var after int
		if err := encoredb.DB.QueryRow(ctx,
			`SELECT COUNT(*) FROM deps.dependencies WHERE from_item = $1 OR to_item = $1`, ft.ClaimedID,
		).Scan(&after); err != nil {
			t.Fatalf("count foreign edges after: %v", err)
		}
		if after != before {
			t.Fatalf("cross-tenant add_dependency created an edge (before=%d after=%d) in foreign org", before, after)
		}
	})

	// --- remove_dependency on a FOREIGN edge ⇒ NOT_FOUND, edge survives --
	t.Run("remove_dependency", func(t *testing.T) {
		env := callTool(t, f.RawKey, sessionID, "remove_dependency", map[string]any{
			"edge_id": ft.EdgeID,
		})
		assertNotFound(t, env, "remove_dependency")
		var n int
		if err := encoredb.DB.QueryRow(ctx,
			`SELECT COUNT(*) FROM deps.dependencies WHERE id = $1`, ft.EdgeID,
		).Scan(&n); err != nil {
			t.Fatalf("count foreign edge after remove: %v", err)
		}
		if n != 1 {
			t.Fatalf("cross-tenant remove_dependency deleted foreign edge (count=%d, want 1)", n)
		}
	})
}

// countCallerItemsTitled counts items in the CALLER's org carrying the given
// title — used to prove a rejected create stored ZERO rows (the gate rolled the
// whole transaction back, never planting a cross-org-referencing row).
func countCallerItemsTitled(t *testing.T, ctx context.Context, orgID, title string) int {
	t.Helper()
	var n int
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT COUNT(*) FROM workitems.items WHERE org_id = $1 AND title = $2`, orgID, title,
	).Scan(&n); err != nil {
		t.Fatalf("count caller items titled %q: %v", title, err)
	}
	return n
}

// countItemComments returns the number of comments on the given item — used to
// prove the AppendComment parent_id same-item gate (bead unblock-tv8.80) inserts
// ZERO rows when a non-empty parent_id is foreign-org or cross-item.
func countItemComments(t *testing.T, ctx context.Context, itemID string) int {
	t.Helper()
	var n int
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT COUNT(*) FROM workitems.comments WHERE item_id = $1`, itemID,
	).Scan(&n); err != nil {
		t.Fatalf("count comments on item %s: %v", itemID, err)
	}
	return n
}

// TestExitCriterion_CreateCrossReference_CrossTenantRejected drives the `create`
// tool under the fixture's Bearer (org A) naming a FOREIGN cross-reference
// (org B) per reference type. Each must yield NOT_FOUND and store ZERO rows in
// the caller's org — the create-path IDOR seam closed by bead unblock-tv8.78
// (SPEC §10.1.1 / §4.4 Create / §6.2 Tool 4). req.OrgID is the gate key; a
// foreign-but-existing reference is indistinguishable from a missing id.
//
// Each case supplies the caller's OWN valid project_id (the handler requires a
// non-empty project_id) and varies exactly one foreign reference — except the
// foreign-project case, where the foreign project_id IS the reference under
// test. type=task keeps parent_id / discovered_from_id OPTIONAL so they can be
// exercised in isolation (a finding would force all of them present at once).
func TestExitCriterion_CreateCrossReference_CrossTenantRejected(t *testing.T) {
	f := fx(t)
	sessionID := initializeSession(t, f.RawKey)
	ctx := t.Context()
	ft := seedForeignTenant(t, ctx)

	// --- foreign project_id ⇒ NOT_FOUND, no item stored ------------------
	t.Run("foreign_project_id", func(t *testing.T) {
		title := "create-foreign-project"
		env := callTool(t, f.RawKey, sessionID, "create", map[string]any{
			"project_id": ft.ProjectID, // foreign — the reference under test
			"type":       "task",
			"title":      title,
		})
		assertNotFound(t, env, "create (foreign project_id)")
		if n := countCallerItemsTitled(t, ctx, f.OrgID, title); n != 0 {
			t.Fatalf("create with foreign project_id stored %d item(s) in caller org, want 0", n)
		}
	})

	// --- foreign parent_id ⇒ NOT_FOUND, no item stored -------------------
	t.Run("foreign_parent_id", func(t *testing.T) {
		title := "create-foreign-parent"
		env := callTool(t, f.RawKey, sessionID, "create", map[string]any{
			"project_id": f.ProjectID, // caller's own valid project
			"type":       "task",
			"title":      title,
			"parent_id":  ft.ItemID, // foreign item — the reference under test
		})
		assertNotFound(t, env, "create (foreign parent_id)")
		if n := countCallerItemsTitled(t, ctx, f.OrgID, title); n != 0 {
			t.Fatalf("create with foreign parent_id stored %d item(s) in caller org, want 0", n)
		}
	})

	// --- foreign discovered_from_id ⇒ NOT_FOUND, no item stored ----------
	t.Run("foreign_discovered_from_id", func(t *testing.T) {
		title := "create-foreign-discovered-from"
		env := callTool(t, f.RawKey, sessionID, "create", map[string]any{
			"project_id":         f.ProjectID,
			"type":               "task",
			"title":              title,
			"discovered_from_id": ft.ItemID, // foreign item — the reference under test
		})
		assertNotFound(t, env, "create (foreign discovered_from_id)")
		if n := countCallerItemsTitled(t, ctx, f.OrgID, title); n != 0 {
			t.Fatalf("create with foreign discovered_from_id stored %d item(s) in caller org, want 0", n)
		}
	})

	// --- foreign milestone_id ⇒ NOT_FOUND, no item stored ----------------
	t.Run("foreign_milestone_id", func(t *testing.T) {
		title := "create-foreign-milestone"
		env := callTool(t, f.RawKey, sessionID, "create", map[string]any{
			"project_id":   f.ProjectID,
			"type":         "task",
			"title":        title,
			"milestone_id": ft.MilestoneID, // foreign milestone — the reference under test
		})
		assertNotFound(t, env, "create (foreign milestone_id)")
		if n := countCallerItemsTitled(t, ctx, f.OrgID, title); n != 0 {
			t.Fatalf("create with foreign milestone_id stored %d item(s) in caller org, want 0", n)
		}
	})

	// --- foreign PROJECT-scoped milestone_id ⇒ NOT_FOUND, no item stored -----
	// The org-scoped case above exercises the org_id = $caller branch of the
	// milestone org-XOR-project gate; this case (org_id NULL, project_id → org
	// B's project) locks the OTHER branch — project_id IN (caller-org projects)
	// — so the full XOR predicate is covered (bead unblock-tv8.78 pre-QA cleanup).
	t.Run("foreign_project_scoped_milestone_id", func(t *testing.T) {
		title := "create-foreign-project-milestone"
		env := callTool(t, f.RawKey, sessionID, "create", map[string]any{
			"project_id":   f.ProjectID, // caller's own valid project
			"type":         "task",
			"title":        title,
			"milestone_id": ft.ProjMilesID, // foreign project-scoped milestone — the reference under test
		})
		assertNotFound(t, env, "create (foreign project-scoped milestone_id)")
		if n := countCallerItemsTitled(t, ctx, f.OrgID, title); n != 0 {
			t.Fatalf("create with foreign project-scoped milestone_id stored %d item(s) in caller org, want 0", n)
		}
	})

	// --- foreign label_id ⇒ NOT_FOUND, no item stored, no item_labels row
	// (the labels[] gate, folded into this bead per Miguel 2026-06-12) ----
	t.Run("foreign_label_id", func(t *testing.T) {
		title := "create-foreign-label"
		env := callTool(t, f.RawKey, sessionID, "create", map[string]any{
			"project_id": f.ProjectID,
			"type":       "task",
			"title":      title,
			"labels":     []string{ft.LabelID}, // foreign label — the reference under test
		})
		assertNotFound(t, env, "create (foreign label_id)")
		if n := countCallerItemsTitled(t, ctx, f.OrgID, title); n != 0 {
			t.Fatalf("create with foreign label_id stored %d item(s) in caller org, want 0", n)
		}
		// The atomic rollback also guarantees no item_labels row references the
		// foreign label — assert the foreign label attached to nothing.
		var n int
		if err := encoredb.DB.QueryRow(ctx,
			`SELECT COUNT(*) FROM workitems.item_labels WHERE label_id = $1`, ft.LabelID,
		).Scan(&n); err != nil {
			t.Fatalf("count item_labels for foreign label: %v", err)
		}
		if n != 0 {
			t.Fatalf("create attached foreign label to %d item(s), want 0", n)
		}
	})
}
