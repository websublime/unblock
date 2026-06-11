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
//   - update            (workitems.Update)
//   - comment           (workitems.AppendComment, INSERT … SELECT gate)
//   - set_state         (workitems.SetStateColumns)
//   - close             (workitems.Close)
//   - claim             (workitems.Claim — NOT_FOUND, never ALREADY_CLAIMED)
//   - promote           (workitems.Promote)
//   - assign_item       (workitems.AssignItem)
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
	MilestoneID string // a foreign org-scoped milestone
	EdgeID      string // a foreign edge ItemID → ClaimedID
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
		EdgeID:      mustULID(t, "foreign edge"),
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
	// A foreign org-scoped milestone.
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO workitems.milestones (id, org_id, name, start_date, end_date)
		 VALUES ($1, $2, 'Foreign Milestone', '2026-01-01', '2026-12-31')`,
		ft.MilestoneID, ft.OrgID,
	); err != nil {
		t.Fatalf("insert foreign milestone: %v", err)
	}
	// A foreign edge between the two foreign items.
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO deps.dependencies (id, from_item, to_item, kind, created_by)
		 VALUES ($1, $2, $3, 'blocks', $4)`,
		ft.EdgeID, ft.ItemID, ft.ClaimedID, ft.UserID,
	); err != nil {
		t.Fatalf("insert foreign edge: %v", err)
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

	// --- comment: foreign item ⇒ NOT_FOUND, zero comments inserted ------
	t.Run("comment", func(t *testing.T) {
		env := callTool(t, f.RawKey, sessionID, "comment", map[string]any{
			"item_id": ft.ItemID,
			"body":    "intruder comment",
		})
		assertNotFound(t, env, "comment")
		var n int
		if err := encoredb.DB.QueryRow(ctx,
			`SELECT COUNT(*) FROM workitems.comments WHERE item_id = $1`, ft.ItemID,
		).Scan(&n); err != nil {
			t.Fatalf("count foreign comments: %v", err)
		}
		if n != 0 {
			t.Fatalf("cross-tenant comment inserted %d row(s) on foreign item, want 0", n)
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
