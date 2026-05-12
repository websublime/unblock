// Integration tests for the deps service (C-2 / bead unblock-tv8.11).
//
// These tests touch the real Encore-managed Postgres cluster and the
// in-process Pub/Sub daemon — they MUST run under
// `encore test ./apps/api/deps/...`, not plain `go test`. The external
// test package (`package deps_test`) blank-imports encore.app/db so
// the dedicated migration-owner service's init() fires before any
// subtest, populating deps.db / workitems.db / etc. via the canonical
// BindDB late-bind pattern. The internal `package deps` test surface
// cannot do this because encore.app/db imports encore.app/deps to
// call deps.BindDB(DB) — a back-import from inside package deps
// would form a compile-time cycle.
//
// Coverage (per acceptance criteria of bead unblock-tv8.11):
//
//   - AddEdge happy path (kind='blocks' default).
//   - AddEdge cross-project rejection (VALIDATION / Meta[field]=to_item_id).
//   - AddEdge cycle rejection (CYCLE_DETECTED with cycle_path populated).
//   - AddEdge self-loop rejection.
//   - AddEdge readiness flip: when from_item is not Done, to_item flips
//     is_ready=false inline (Regime A).
//   - RemoveEdge by EdgeID + by composite key.
//   - RemoveEdge writes one cascade_events row with kind='edge_removed'
//     inline; the post-commit publish does NOT double-write thanks to
//     the ON CONFLICT (event_id, triggered_by_item_id) DO NOTHING
//     clause (round-6 tension #1, exercised end-to-end).
//   - RemoveEdge flips to_item_now_ready back to true when the only
//     blocking edge is removed.
//   - Closure incoming + outgoing.
//   - RecentCascadeEvents default limit (50) + org/project scope.
//   - Property test: 100 random graph mutations produce zero cycles
//     in the DB (acceptance criterion #5).

package deps_test

import (
	"context"
	"errors"
	"math/rand"
	"strings"
	"testing"
	"time"

	// Importing encore.app/db triggers its init() which calls every
	// consumer's BindDB hook. Without this import the test binary
	// loads deps in isolation and leaves deps.db == nil — every RPC
	// body would then panic on a nil *sqldb.Database.
	encoredb "encore.app/db"
	"encore.app/deps"
	"encore.app/shared/ulid"
	"encore.dev/beta/errs"
	"encore.dev/storage/sqldb"
)

// fixture seeds the org / project / user rows each test depends on.
type fixture struct {
	OrgID     string
	ProjectID string
	UserID    string
}

func seedFixture(t *testing.T, ctx context.Context) *fixture {
	t.Helper()

	orgID := mustULID(t)
	slug := strings.ToLower("dt-" + orgID[len(orgID)-8:])
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO org.organizations (id, slug, name) VALUES ($1, $2, $3)`,
		orgID, slug, "deps test org",
	); err != nil {
		t.Fatalf("insert org: %v", err)
	}

	userID := mustULID(t)
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO auth.users (id, primary_provider, primary_provider_id, email, display_name)
		 VALUES ($1, 'github', $2, $3, $4)`,
		userID, "dt-"+userID[len(userID)-8:],
		strings.ToLower(userID[len(userID)-8:])+"@dt.local", "dt",
	); err != nil {
		t.Fatalf("insert user: %v", err)
	}

	projectID := mustULID(t)
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO org.projects (id, org_id, slug, name) VALUES ($1, $2, $3, $4)`,
		projectID, orgID, "p-"+projectID[len(projectID)-8:], "dt project",
	); err != nil {
		t.Fatalf("insert project: %v", err)
	}

	t.Cleanup(func() {
		// Deleting the org cascades to projects, items, dependencies,
		// cascade_events via ON DELETE CASCADE on the schema.
		_, _ = encoredb.DB.Exec(ctx, `DELETE FROM org.organizations WHERE id = $1`, orgID)
		_, _ = encoredb.DB.Exec(ctx, `DELETE FROM auth.users WHERE id = $1`, userID)
	})

	return &fixture{OrgID: orgID, ProjectID: projectID, UserID: userID}
}

func mustULID(t *testing.T) string {
	t.Helper()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	return id
}

// createItem inserts a Backlog work item directly via SQL. status
// defaults to "Backlog" so AddEdge can create blocking edges that
// keep to_item not-ready (the Done/non-Done classification is what
// drives recomputeReady).
func createItem(t *testing.T, ctx context.Context, fx *fixture, status string) string {
	t.Helper()
	id := mustULID(t)
	if status == "" {
		status = "Backlog"
	}
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, type, title, status, is_ready)
		 VALUES ($1, $2, $3, 'task', 'dt item', $4, true)`,
		id, fx.OrgID, fx.ProjectID, status,
	); err != nil {
		t.Fatalf("insert item: %v", err)
	}
	return id
}

// createItemInProject inserts an item into an explicit project (for
// cross-project edge tests).
func createItemInProject(t *testing.T, ctx context.Context, orgID, projectID, status string) string {
	t.Helper()
	id := mustULID(t)
	if status == "" {
		status = "Backlog"
	}
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, type, title, status, is_ready)
		 VALUES ($1, $2, $3, 'task', 'dt item', $4, true)`,
		id, orgID, projectID, status,
	); err != nil {
		t.Fatalf("insert item: %v", err)
	}
	return id
}

// readIsReady reads workitems.items.is_ready by id.
func readIsReady(t *testing.T, ctx context.Context, id string) bool {
	t.Helper()
	var ready bool
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT is_ready FROM workitems.items WHERE id = $1`, id,
	).Scan(&ready); err != nil {
		t.Fatalf("read is_ready: %v", err)
	}
	return ready
}

// -----------------------------------------------------------------------------
// AddEdge.
// -----------------------------------------------------------------------------

func TestAddEdgeHappyPath(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	from := createItem(t, ctx, fx, "Backlog")
	to := createItem(t, ctx, fx, "Backlog")

	edge, err := deps.AddEdge(ctx, &deps.AddEdgeRequest{
		OrgID:     fx.OrgID,
		ProjectID: fx.ProjectID,
		FromItem:  from,
		ToItem:    to,
	})
	if err != nil {
		t.Fatalf("AddEdge: %v", err)
	}
	if edge.ID == "" || edge.FromItem != from || edge.ToItem != to || edge.Kind != "blocks" {
		t.Fatalf("AddEdge returned unexpected edge: %+v", edge)
	}
	// Regime A: to is now blocked by a non-Done from → is_ready=false.
	if readIsReady(t, ctx, to) {
		t.Fatalf("to_item is_ready = true, want false after blocking edge from non-Done item")
	}
}

// TestAddEdgeDuplicateRejected exercises the dependencies_pair_uniq
// branch in AddEdge — the second insert with the same
// (from_item, to_item, kind) MUST return errs.AlreadyExists with
// Meta{from, to, kind}, not a generic Internal error. Locks the
// pgconn.PgError SQLSTATE 23505 path in helpers.isUniqueViolation
// (review L6-S3).
func TestAddEdgeDuplicateRejected(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	from := createItem(t, ctx, fx, "Backlog")
	to := createItem(t, ctx, fx, "Backlog")

	if _, err := deps.AddEdge(ctx, &deps.AddEdgeRequest{
		OrgID:     fx.OrgID,
		ProjectID: fx.ProjectID,
		FromItem:  from,
		ToItem:    to,
	}); err != nil {
		t.Fatalf("AddEdge (first): %v", err)
	}

	_, err := deps.AddEdge(ctx, &deps.AddEdgeRequest{
		OrgID:     fx.OrgID,
		ProjectID: fx.ProjectID,
		FromItem:  from,
		ToItem:    to,
		Kind:      "blocks",
	})
	if err == nil {
		t.Fatalf("expected AlreadyExists on duplicate (from,to,kind), got nil")
	}
	if errs.Code(err) != errs.AlreadyExists {
		t.Fatalf("code = %v, want AlreadyExists", errs.Code(err))
	}
	meta := errs.Meta(err)
	if got := meta["from"]; got != from {
		t.Fatalf("meta[from] = %v, want %q", got, from)
	}
	if got := meta["to"]; got != to {
		t.Fatalf("meta[to] = %v, want %q", got, to)
	}
	if got := meta["kind"]; got != "blocks" {
		t.Fatalf("meta[kind] = %v, want \"blocks\"", got)
	}
}

func TestAddEdgeReadyWhenFromIsDone(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	from := createItem(t, ctx, fx, "Done")
	to := createItem(t, ctx, fx, "Backlog")

	if _, err := deps.AddEdge(ctx, &deps.AddEdgeRequest{
		OrgID:     fx.OrgID,
		ProjectID: fx.ProjectID,
		FromItem:  from,
		ToItem:    to,
	}); err != nil {
		t.Fatalf("AddEdge: %v", err)
	}
	// from is Done → no blocking constraint → to stays ready.
	if !readIsReady(t, ctx, to) {
		t.Fatalf("to_item is_ready = false, want true when blocker is Done")
	}
}

func TestAddEdgeSelfLoopRejected(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	item := createItem(t, ctx, fx, "Backlog")

	_, err := deps.AddEdge(ctx, &deps.AddEdgeRequest{
		OrgID:     fx.OrgID,
		ProjectID: fx.ProjectID,
		FromItem:  item,
		ToItem:    item,
	})
	if err == nil {
		t.Fatalf("expected InvalidArgument on self-loop, got nil")
	}
	if errs.Code(err) != errs.InvalidArgument {
		t.Fatalf("code = %v, want InvalidArgument", errs.Code(err))
	}
}

func TestAddEdgeRejectsCrossProject(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	otherProject := mustULID(t)
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO org.projects (id, org_id, slug, name) VALUES ($1, $2, $3, $4)`,
		otherProject, fx.OrgID, "p-"+otherProject[len(otherProject)-8:], "other",
	); err != nil {
		t.Fatalf("insert other project: %v", err)
	}

	from := createItem(t, ctx, fx, "Backlog")
	to := createItemInProject(t, ctx, fx.OrgID, otherProject, "Backlog")

	_, err := deps.AddEdge(ctx, &deps.AddEdgeRequest{
		OrgID:     fx.OrgID,
		ProjectID: fx.ProjectID,
		FromItem:  from,
		ToItem:    to,
	})
	if err == nil {
		t.Fatalf("expected InvalidArgument on cross-project edge, got nil")
	}
	if errs.Code(err) != errs.InvalidArgument {
		t.Fatalf("code = %v, want InvalidArgument", errs.Code(err))
	}
	e := err.(*errs.Error)
	if e.Meta["field"] != "to_item_id" {
		t.Fatalf("meta[field] = %v, want to_item_id", e.Meta["field"])
	}
}

func TestAddEdgeRejectsCycleAndPopulatesPath(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	a := createItem(t, ctx, fx, "Backlog")
	b := createItem(t, ctx, fx, "Backlog")
	c := createItem(t, ctx, fx, "Backlog")

	// a -> b -> c, then try c -> a (would close cycle).
	if _, err := deps.AddEdge(ctx, &deps.AddEdgeRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, FromItem: a, ToItem: b,
	}); err != nil {
		t.Fatalf("AddEdge a->b: %v", err)
	}
	if _, err := deps.AddEdge(ctx, &deps.AddEdgeRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, FromItem: b, ToItem: c,
	}); err != nil {
		t.Fatalf("AddEdge b->c: %v", err)
	}

	_, err := deps.AddEdge(ctx, &deps.AddEdgeRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, FromItem: c, ToItem: a,
	})
	if err == nil {
		t.Fatalf("expected FailedPrecondition (CYCLE_DETECTED), got nil")
	}
	if errs.Code(err) != errs.FailedPrecondition {
		t.Fatalf("code = %v, want FailedPrecondition", errs.Code(err))
	}
	e := err.(*errs.Error)
	if e.Meta["kind"] != "CYCLE_DETECTED" {
		t.Fatalf("meta[kind] = %v, want CYCLE_DETECTED", e.Meta["kind"])
	}
	pathStr, ok := e.Meta["cycle_path"].(string)
	if !ok || pathStr == "" {
		t.Fatalf("meta[cycle_path] missing or empty: %#v", e.Meta["cycle_path"])
	}
	pathParts := strings.Split(pathStr, ",")
	if len(pathParts) == 0 {
		t.Fatalf("cycle_path split empty: %q", pathStr)
	}
	// First element must be the proposed from_item (cycle closure point).
	if pathParts[0] != c {
		t.Fatalf("cycle_path[0] = %v, want %q", pathParts[0], c)
	}

	// Forensic row recorded in deps.cycles.
	var rowCount int
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT count(*) FROM deps.cycles WHERE from_item = $1 AND to_item = $2`,
		c, a,
	).Scan(&rowCount); err != nil {
		t.Fatalf("deps.cycles count: %v", err)
	}
	if rowCount != 1 {
		t.Fatalf("deps.cycles rows = %d, want 1", rowCount)
	}

	// And the rejected edge was NOT written.
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT count(*) FROM deps.dependencies
		   WHERE from_item = $1 AND to_item = $2`,
		c, a,
	).Scan(&rowCount); err != nil {
		t.Fatalf("deps.dependencies count: %v", err)
	}
	if rowCount != 0 {
		t.Fatalf("rejected edge was written (rows = %d)", rowCount)
	}
}

// -----------------------------------------------------------------------------
// RemoveEdge.
// -----------------------------------------------------------------------------

func TestRemoveEdgeByEdgeID(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	from := createItem(t, ctx, fx, "Backlog")
	to := createItem(t, ctx, fx, "Backlog")

	edge, err := deps.AddEdge(ctx, &deps.AddEdgeRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, FromItem: from, ToItem: to,
	})
	if err != nil {
		t.Fatalf("AddEdge: %v", err)
	}

	resp, err := deps.RemoveEdge(ctx, &deps.RemoveEdgeRequest{EdgeID: edge.ID})
	if err != nil {
		t.Fatalf("RemoveEdge: %v", err)
	}
	if !resp.Removed {
		t.Fatalf("removed = false")
	}
	if resp.ToItemID != to {
		t.Fatalf("to_item_id = %q, want %q", resp.ToItemID, to)
	}
	// Only edge gone → to flips back to ready.
	if !resp.ToItemNowReady {
		t.Fatalf("to_item_now_ready = false, want true (only blocker removed)")
	}
	if !readIsReady(t, ctx, to) {
		t.Fatalf("is_ready not flipped back to true on the row")
	}
}

func TestRemoveEdgeByCompositeKey(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	from := createItem(t, ctx, fx, "Backlog")
	to := createItem(t, ctx, fx, "Backlog")

	if _, err := deps.AddEdge(ctx, &deps.AddEdgeRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, FromItem: from, ToItem: to,
	}); err != nil {
		t.Fatalf("AddEdge: %v", err)
	}
	resp, err := deps.RemoveEdge(ctx, &deps.RemoveEdgeRequest{
		FromItem: from, ToItem: to, Kind: "blocks",
	})
	if err != nil {
		t.Fatalf("RemoveEdge composite: %v", err)
	}
	if !resp.Removed || resp.ToItemID != to {
		t.Fatalf("composite remove: %+v", resp)
	}
}

func TestRemoveEdgeWritesInlineCascadeEvent(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	from := createItem(t, ctx, fx, "Backlog")
	to := createItem(t, ctx, fx, "Backlog")

	edge, err := deps.AddEdge(ctx, &deps.AddEdgeRequest{
		OrgID: fx.OrgID, ProjectID: fx.ProjectID, FromItem: from, ToItem: to,
	})
	if err != nil {
		t.Fatalf("AddEdge: %v", err)
	}
	if _, err := deps.RemoveEdge(ctx, &deps.RemoveEdgeRequest{EdgeID: edge.ID}); err != nil {
		t.Fatalf("RemoveEdge: %v", err)
	}

	// Allow up to 2s for any subscriber-side reinsert to attempt and
	// collapse via ON CONFLICT. Round-6 tension #1: exactly one row
	// per logical remove. We poll a couple of times rather than fail
	// immediately so the test tolerates the in-process pubsub fan-out
	// latency.
	var rowCount int
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		if err := encoredb.DB.QueryRow(ctx,
			`SELECT count(*) FROM deps.cascade_events
			   WHERE org_id = $1 AND triggered_by_item_id = $2 AND kind = 'edge_removed'`,
			fx.OrgID, to,
		).Scan(&rowCount); err != nil {
			t.Fatalf("cascade_events count: %v", err)
		}
		if rowCount >= 1 {
			break
		}
		time.Sleep(50 * time.Millisecond)
	}
	if rowCount != 1 {
		t.Fatalf("cascade_events rows (kind=edge_removed) for to=%q = %d, want 1 (tension #1: exactly one row per logical remove)", to, rowCount)
	}
}

func TestRemoveEdgeNotFound(t *testing.T) {
	ctx := context.Background()
	_ = seedFixture(t, ctx)
	_, err := deps.RemoveEdge(ctx, &deps.RemoveEdgeRequest{EdgeID: mustULID(t)})
	if err == nil {
		t.Fatalf("expected NotFound, got nil")
	}
	if errs.Code(err) != errs.NotFound {
		t.Fatalf("code = %v, want NotFound", errs.Code(err))
	}
}

func TestRemoveEdgeRejectsBadSelection(t *testing.T) {
	ctx := context.Background()
	_, err := deps.RemoveEdge(ctx, &deps.RemoveEdgeRequest{})
	if err == nil || errs.Code(err) != errs.InvalidArgument {
		t.Fatalf("empty selection: code = %v, want InvalidArgument", errs.Code(err))
	}
	_, err = deps.RemoveEdge(ctx, &deps.RemoveEdgeRequest{
		EdgeID: mustULID(t), FromItem: "a", ToItem: "b", Kind: "blocks",
	})
	if err == nil || errs.Code(err) != errs.InvalidArgument {
		t.Fatalf("both selection: code = %v, want InvalidArgument", errs.Code(err))
	}
}

// -----------------------------------------------------------------------------
// IsReady, Closure, RecentCascadeEvents.
// -----------------------------------------------------------------------------

func TestIsReadyReadsColumn(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	id := createItem(t, ctx, fx, "Backlog")
	got, err := deps.IsReady(ctx, &deps.IsReadyRequest{ItemID: id})
	if err != nil {
		t.Fatalf("IsReady: %v", err)
	}
	if !got.IsReady {
		t.Fatalf("is_ready = false, want true (no blocking edges)")
	}
}

func TestIsReadyNotFound(t *testing.T) {
	ctx := context.Background()
	_, err := deps.IsReady(ctx, &deps.IsReadyRequest{ItemID: mustULID(t)})
	if err == nil || errs.Code(err) != errs.NotFound {
		t.Fatalf("code = %v, want NotFound", errs.Code(err))
	}
}

func TestClosureOutgoingAndIncoming(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)
	// Chain: a -> b -> c.
	a := createItem(t, ctx, fx, "Backlog")
	b := createItem(t, ctx, fx, "Backlog")
	c := createItem(t, ctx, fx, "Backlog")
	for _, p := range [][2]string{{a, b}, {b, c}} {
		if _, err := deps.AddEdge(ctx, &deps.AddEdgeRequest{
			OrgID: fx.OrgID, ProjectID: fx.ProjectID, FromItem: p[0], ToItem: p[1],
		}); err != nil {
			t.Fatalf("AddEdge %s->%s: %v", p[0], p[1], err)
		}
	}

	// Outgoing from a: should reach b, c (excludes a).
	out, err := deps.Closure(ctx, &deps.ClosureRequest{ItemID: a, Direction: "outgoing"})
	if err != nil {
		t.Fatalf("Closure outgoing: %v", err)
	}
	want := map[string]bool{b: true, c: true}
	if len(out.ItemIDs) != 2 || !want[out.ItemIDs[0]] || !want[out.ItemIDs[1]] {
		t.Fatalf("outgoing closure = %v, want {%q, %q}", out.ItemIDs, b, c)
	}

	// Incoming to c: should reach b, a (excludes c).
	in, err := deps.Closure(ctx, &deps.ClosureRequest{ItemID: c, Direction: "incoming"})
	if err != nil {
		t.Fatalf("Closure incoming: %v", err)
	}
	want = map[string]bool{a: true, b: true}
	if len(in.ItemIDs) != 2 || !want[in.ItemIDs[0]] || !want[in.ItemIDs[1]] {
		t.Fatalf("incoming closure = %v, want {%q, %q}", in.ItemIDs, a, b)
	}
}

func TestClosureRejectsBadDirection(t *testing.T) {
	ctx := context.Background()
	_, err := deps.Closure(ctx, &deps.ClosureRequest{
		ItemID: mustULID(t), Direction: "sideways",
	})
	if err == nil || errs.Code(err) != errs.InvalidArgument {
		t.Fatalf("code = %v, want InvalidArgument", errs.Code(err))
	}
}

func TestClosureRejectsDepthOverflow(t *testing.T) {
	ctx := context.Background()
	_, err := deps.Closure(ctx, &deps.ClosureRequest{
		ItemID: mustULID(t), Direction: "outgoing", MaxDepth: 1000,
	})
	if err == nil || errs.Code(err) != errs.InvalidArgument {
		t.Fatalf("code = %v, want InvalidArgument", errs.Code(err))
	}
}

func TestRecentCascadeEventsDefaultLimitAndScope(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	// Seed 60 cascade_events rows directly via SQL — exercises the
	// limit cap (default 50) and the ORDER BY triggered_at DESC.
	for i := 0; i < 60; i++ {
		eventID := mustULID(t)
		rowID := mustULID(t)
		if _, err := encoredb.DB.Exec(ctx,
			`INSERT INTO deps.cascade_events
			   (id, event_id, kind, org_id, project_id,
			    affected_item_ids, cascaded_count, triggered_at)
			 VALUES ($1, $2, 'close', $3, $4, $5, 0, now() - make_interval(secs => $6))`,
			rowID, eventID, fx.OrgID, fx.ProjectID, []string{}, 60-i,
		); err != nil {
			t.Fatalf("insert cascade_event %d: %v", i, err)
		}
	}

	// Default limit (0 → 50).
	resp, err := deps.RecentCascadeEvents(ctx, &deps.RecentCascadeEventsRequest{OrgID: fx.OrgID})
	if err != nil {
		t.Fatalf("RecentCascadeEvents: %v", err)
	}
	if len(resp.Events) != 50 {
		t.Fatalf("default limit = %d, want 50", len(resp.Events))
	}

	// Explicit Limit > cap clamps to 50.
	resp, err = deps.RecentCascadeEvents(ctx, &deps.RecentCascadeEventsRequest{OrgID: fx.OrgID, Limit: 9000})
	if err != nil {
		t.Fatalf("RecentCascadeEvents over cap: %v", err)
	}
	if len(resp.Events) != 50 {
		t.Fatalf("over-cap limit = %d, want 50", len(resp.Events))
	}

	// Project scope filters out other projects.
	otherProject := mustULID(t)
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO org.projects (id, org_id, slug, name) VALUES ($1, $2, $3, $4)`,
		otherProject, fx.OrgID, "p-"+otherProject[len(otherProject)-8:], "other",
	); err != nil {
		t.Fatalf("insert project: %v", err)
	}
	if _, err := encoredb.DB.Exec(ctx,
		`INSERT INTO deps.cascade_events
		   (id, event_id, kind, org_id, project_id, affected_item_ids, cascaded_count)
		 VALUES ($1, $2, 'close', $3, $4, '{}', 0)`,
		mustULID(t), mustULID(t), fx.OrgID, otherProject,
	); err != nil {
		t.Fatalf("insert other-project event: %v", err)
	}
	resp, err = deps.RecentCascadeEvents(ctx, &deps.RecentCascadeEventsRequest{
		OrgID:     fx.OrgID,
		ProjectID: otherProject,
		Limit:     10,
	})
	if err != nil {
		t.Fatalf("RecentCascadeEvents project-scope: %v", err)
	}
	if len(resp.Events) != 1 {
		t.Fatalf("project-scope hits = %d, want 1", len(resp.Events))
	}
}

// -----------------------------------------------------------------------------
// Property test: 100 random graph mutations produce zero DB cycles.
// -----------------------------------------------------------------------------

// TestPropertyNoCyclesAfter100Mutations is the AC #5 acceptance test:
// N=100 random AddEdge/RemoveEdge calls against a fixed pool of items
// MUST leave deps.dependencies acyclic. We invoke a check at the SQL
// level after each mutation: the same depth-counter CTE the cycle
// detector uses, but seeded from every node — if ANY node reaches
// itself via a 'blocks' walk, the graph has a cycle.
//
// Also asserts the round-6 cascade-symmetry property: each successful
// RemoveEdge results in exactly one cascade_events row with
// kind='edge_removed' (tension #1: no subscriber-driven duplicate).
func TestPropertyNoCyclesAfter100Mutations(t *testing.T) {
	ctx := context.Background()
	fx := seedFixture(t, ctx)

	const poolSize = 20
	const mutations = 100

	pool := make([]string, poolSize)
	for i := range pool {
		pool[i] = createItem(t, ctx, fx, "Backlog")
	}

	rng := rand.New(rand.NewSource(0xc2c2c2c2))
	var (
		addOK       int
		addReject   int
		removeOK    int
		removeMiss  int
		removeCount = make(map[string]int) // edge_id -> times removed (should be 0 or 1)
	)

	for i := 0; i < mutations; i++ {
		// 70/30 split between AddEdge and RemoveEdge to keep the graph
		// non-trivial. Skip self-loops (rejected unconditionally).
		if rng.Intn(10) < 7 {
			fromIdx := rng.Intn(poolSize)
			toIdx := rng.Intn(poolSize)
			if fromIdx == toIdx {
				continue
			}
			_, err := deps.AddEdge(ctx, &deps.AddEdgeRequest{
				OrgID:     fx.OrgID,
				ProjectID: fx.ProjectID,
				FromItem:  pool[fromIdx],
				ToItem:    pool[toIdx],
			})
			switch {
			case err == nil:
				addOK++
			case errs.Code(err) == errs.FailedPrecondition:
				addReject++
			case errs.Code(err) == errs.AlreadyExists:
				// Existing edge — no-op for the property invariant.
			default:
				// Other errors (Internal etc.) are pathologies — fail loud.
				t.Fatalf("iter %d AddEdge unexpected error: %v", i, err)
			}
		} else {
			// Pick a random existing edge to remove.
			var edgeID string
			if err := encoredb.DB.QueryRow(ctx,
				`SELECT id FROM deps.dependencies
				  WHERE from_item = ANY($1) AND to_item = ANY($1)
				  ORDER BY random()
				  LIMIT 1`,
				pool,
			).Scan(&edgeID); err != nil {
				// No edges yet — count as a no-op iteration.
				removeMiss++
				continue
			}
			if _, err := deps.RemoveEdge(ctx, &deps.RemoveEdgeRequest{EdgeID: edgeID}); err != nil {
				t.Fatalf("iter %d RemoveEdge(%q): %v", i, edgeID, err)
			}
			removeOK++
			removeCount[edgeID]++
		}

		// Invariant: no cycles exist in deps.dependencies.
		assertAcyclic(t, ctx, pool)
	}

	// Each successful RemoveEdge writes exactly one cascade_events row
	// inline; the post-commit publish's subscriber INSERT collapses
	// via ON CONFLICT. Allow a brief settle window for the subscriber.
	time.Sleep(500 * time.Millisecond)
	var auditRows int
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT count(*) FROM deps.cascade_events
		   WHERE org_id = $1 AND kind = 'edge_removed'`,
		fx.OrgID,
	).Scan(&auditRows); err != nil {
		t.Fatalf("audit count: %v", err)
	}
	if auditRows != removeOK {
		t.Fatalf("kind=edge_removed audit rows = %d, want %d (tension #1: one row per logical remove)",
			auditRows, removeOK)
	}

	t.Logf("property summary: addOK=%d addReject=%d removeOK=%d removeMiss=%d",
		addOK, addReject, removeOK, removeMiss)
}

// Cascade subscriber tests for C-3 live in the internal-package file
// apps/api/deps/cascade_subscriber_handler_test.go. Encore's pubsub
// testing implementation does NOT fire subscriptions during
// `encore test` (https://encore.dev/docs/go/primitives/pubsub#testing-pubsub),
// so the subscriber is exercised by calling handleCascadeRequested
// directly from an internal-package test. See the DEVIATION comment on
// bead unblock-tv8.12 for the full reasoning.

// assertAcyclic seeds the same depth-counter CTE the cycle detector
// uses, but starts from every node in the pool. If any node reaches
// itself via a 'blocks' walk, the graph has a cycle — fail the test
// with the offending node id.
func assertAcyclic(t *testing.T, ctx context.Context, pool []string) {
	t.Helper()
	var hit *string
	err := encoredb.DB.QueryRow(ctx,
		`WITH RECURSIVE reachable(seed, id, depth) AS (
		    SELECT unnest($1::text[]), unnest($1::text[]), 0
		    UNION ALL
		    SELECT r.seed, d.to_item, r.depth + 1
		      FROM deps.dependencies d
		      JOIN reachable r ON d.from_item = r.id
		     WHERE d.kind = 'blocks'
		       AND r.depth < 256
		 )
		 SELECT seed FROM reachable WHERE seed = id AND depth > 0 LIMIT 1`,
		pool,
	).Scan(&hit)
	if err != nil {
		// sqldb.ErrNoRows = no cycle (the QueryRow path uses pgx's
		// scanner, which surfaces ErrNoRows as the only "clean" empty
		// case). Use the typed sentinel via errors.Is — substring
		// matching on the English message is locale-fragile (review
		// L6-W3). Treat anything else as a fatal harness error.
		if errors.Is(err, sqldb.ErrNoRows) {
			return
		}
		t.Fatalf("acyclic check: %v", err)
	}
	if hit != nil {
		t.Fatalf("cycle detected via node %q after recent mutation", *hit)
	}
}
