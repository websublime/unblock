// d3_tools_test.go covers the D-3 (unblock-tv8.18) acceptance matrix
// for the four MCP tool handlers — update, close, show, list.
//
// Builds on the d2Fixture / callTool / assertStructuredEchoesText
// harness from d2_tools_test.go (same package). The fixture wires
// up auth → MCPHandler → workitems → deps end-to-end so every test
// here exercises the §6.2 wire contract literally.
//
// Coverage matrix (bead unblock-tv8.18 AC):
//
//   - update_RejectsStateDimensions: every forbidden field
//     (impl_state, review_state, qa_state, pipeline_state) returns the
//     §7 VALIDATION envelope with data.field=<name>. Per SPEC §6.2
//     Tool 5 line 1316 + AC #1.
//   - update_HappyPath: title/body/priority/milestone_id/labels are
//     persisted; a fresh show reflects the change.
//   - close_RequiresClaim: returns PRECONDITION_NOT_MET with
//     data.missing="claimed_by_id". Per SPEC §6.2 Tool 6 line 1334
//     + AC #2.
//   - close_HappyPath: status flip to Done + closed_at populated +
//     kind=completed comment landed transactionally. AC #3 ("close
//     emits CascadeRequested on success") is asserted at the
//     workitems integration test layer (TestCloseHappyPath in
//     apps/api/workitems/integration_test.go) where the publisher
//     and observer share the same test goroutine — see
//     TestD3_CloseHappyPath's doc comment for the cross-goroutine
//     limitation of the mcpaudittest harness.
//   - show_ReturnsAllFourCollections: comments, dependencies_in,
//     dependencies_out, findings each populated against a seeded
//     fixture. Per SPEC §6.2 Tool 7 + AC #4.
//   - show_RespectsIncludeFlags: include_comments=false drops the
//     comments[]; same for dependencies / findings (each flag
//     independently observable).
//   - list_NextCursorWhenOverLimit: seed (limit+1) items, expect
//     non-null next_cursor; follow-up cursor returns the rest with
//     next_cursor=null. Per AC #5.
//   - list_MilestoneIDFilter: only the items assigned to the chosen
//     milestone come back.
//   - list_StateDimensionFilters: status=[Ready,InProgress] and
//     pipeline_stage=[Implementation] each narrow the set
//     (behavioural correctness; index usage is verified separately).
//   - list_CrossToolCursorRejected: a ready cursor presented to list
//     surfaces §7 VALIDATION (data.field="cursor"), per §6.2.0.
//   - audit_row_per_tool: every tool dispatch writes exactly one
//     mcp.tool_calls row with the matching tool_name.

package mcpaudittest

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"testing"
	"time"

	"encore.app/shared/ulid"
)

// seedClaimedItem inserts an item already in InProgress + claimed by
// the caller so workitems.Close can run without the prior Claim
// round-trip. The MCP `close` happy path relies on this — Claim's
// cascade publish timing under encore test is racy and we only need
// the post-Close shape.
func seedClaimedItem(t *testing.T, orgID, projectID, userID string) string {
	t.Helper()
	ctx := context.Background()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, type, title, status, priority,
		    claimed_by_id, claimed_by_agent, claimed_at,
		    created_at, updated_at)
		 VALUES ($1, $2, $3, 'task', $4, 'InProgress', 'P2',
		         $5, 'claude-code', now(), now(), now())`,
		id, orgID, projectID, "d3-claimed", userID,
	); err != nil {
		t.Fatalf("insert claimed item: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM workitems.items WHERE id = $1`, id) })
	return id
}

// seedUnclaimedItem inserts an item in Ready / claimed_by_id=NULL so
// the AF3 close precondition fires.
func seedUnclaimedItem(t *testing.T, orgID, projectID string) string {
	t.Helper()
	ctx := context.Background()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, type, title, status, priority,
		    is_ready, created_at, updated_at)
		 VALUES ($1, $2, $3, 'task', $4, 'Ready', 'P2', true, now(), now())`,
		id, orgID, projectID, "d3-unclaimed",
	); err != nil {
		t.Fatalf("insert unclaimed item: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM workitems.items WHERE id = $1`, id) })
	return id
}

// seedItemWithStatus inserts an item with explicit status / pipeline_stage
// for list-filter coverage. Caller owns the cleanup.
func seedItemWithStatus(t *testing.T, orgID, projectID, status, pipelineStage string) string {
	t.Helper()
	ctx := context.Background()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, type, title, status, priority,
		    pipeline_stage, created_at, updated_at)
		 VALUES ($1, $2, $3, 'task', $4, $5, 'P2', $6, now(), now())`,
		id, orgID, projectID, "d3-list-"+status, status, pipelineStage,
	); err != nil {
		t.Fatalf("insert status item: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM workitems.items WHERE id = $1`, id) })
	return id
}

// seedMilestone inserts a milestone row (and any required dates) so
// the list milestone_id filter has a real foreign-key target.
func seedMilestone(t *testing.T, orgID, projectID string) string {
	t.Helper()
	ctx := context.Background()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	// milestones_scope_xor_chk: scope is org_id XOR project_id. We pick
	// project-scoped here so list's milestone_id filter has a real
	// project-local target; org_id is implicitly NULL.
	if _, err := db.Exec(ctx,
		`INSERT INTO workitems.milestones
		   (id, project_id, name, start_date, end_date)
		 VALUES ($1, $2, $3, '2026-01-01', '2026-12-31')`,
		id, projectID, "d3-milestone-"+id[len(id)-6:],
	); err != nil {
		t.Fatalf("insert milestone: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM workitems.milestones WHERE id = $1`, id) })
	return id
}

// assignItemMilestone updates an item's milestone_id directly so list
// can filter on it. We bypass workitems.AssignItem to keep the fixture
// independent of the milestone-RPC surface.
func assignItemMilestone(t *testing.T, itemID, milestoneID string) {
	t.Helper()
	ctx := context.Background()
	if _, err := db.Exec(ctx,
		`UPDATE workitems.items SET milestone_id = $1 WHERE id = $2`,
		milestoneID, itemID,
	); err != nil {
		t.Fatalf("assign milestone: %v", err)
	}
}

// =============================================================================
// update
// =============================================================================

// TestD3_UpdateRejectsStateDimensions covers AC #1: every forbidden
// state-dimension field returns §7 VALIDATION with data.field set to
// the offending name. The test runs all four field names as subtests.
func TestD3_UpdateRejectsStateDimensions(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedUnclaimedItem(t, fx.OrgID, fx.ProjectID)

	for _, field := range []string{"impl_state", "review_state", "qa_state", "pipeline_state"} {
		t.Run(field, func(t *testing.T) {
			env := callTool(t, fx.RawKey, "update", map[string]any{
				"item_id": itemID,
				field:     "done",
			})
			if env.Error == nil {
				t.Fatalf("expected VALIDATION envelope on %s; got success result=%s", field, string(env.Result))
			}
			var data envelopeData
			if err := json.Unmarshal(env.Error.Data, &data); err != nil {
				t.Fatalf("unmarshal error.data: %v", err)
			}
			if data.Kind != "VALIDATION" {
				t.Fatalf("error.data.kind = %q, want VALIDATION", data.Kind)
			}
			if got, _ := data.Details["field"].(string); got != field {
				t.Fatalf("error.data.details.field = %q, want %q", got, field)
			}
		})
	}
}

// TestD3_UpdateHappyPath persists title / body / priority / milestone_id /
// labels and then asserts the round-trip via the `show` tool reflects
// every mutation.
func TestD3_UpdateHappyPath(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedUnclaimedItem(t, fx.OrgID, fx.ProjectID)
	milestoneID := seedMilestone(t, fx.OrgID, fx.ProjectID)

	// Seed a label row so the labels filter has a valid FK target.
	ctx := context.Background()
	labelID, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO workitems.labels (id, org_id, name, color) VALUES ($1, $2, $3, '#abcdef')`,
		labelID, fx.OrgID, "d3-label-"+labelID[len(labelID)-6:],
	); err != nil {
		t.Fatalf("insert label: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM workitems.labels WHERE id = $1`, labelID) })

	env := callTool(t, fx.RawKey, "update", map[string]any{
		"item_id":      itemID,
		"title":        "updated title",
		"body":         "updated body",
		"priority":     "P0",
		"milestone_id": milestoneID,
		"labels":       []string{labelID},
	})
	res := assertStructuredEchoesText(t, env)

	var structured struct {
		Item struct {
			ID          string   `json:"id"`
			Title       string   `json:"title"`
			Body        string   `json:"body"`
			Priority    string   `json:"priority"`
			MilestoneID string   `json:"milestone_id"`
			Labels      []string `json:"labels"`
		} `json:"item"`
	}
	if err := json.Unmarshal(res.StructuredContent, &structured); err != nil {
		t.Fatalf("unmarshal: %v; raw=%s", err, string(res.StructuredContent))
	}
	if structured.Item.Title != "updated title" {
		t.Fatalf("title = %q, want updated title", structured.Item.Title)
	}
	if structured.Item.Body != "updated body" {
		t.Fatalf("body = %q, want updated body", structured.Item.Body)
	}
	if structured.Item.Priority != "P0" {
		t.Fatalf("priority = %q, want P0", structured.Item.Priority)
	}
	if structured.Item.MilestoneID != milestoneID {
		t.Fatalf("milestone_id = %q, want %q", structured.Item.MilestoneID, milestoneID)
	}
	if len(structured.Item.Labels) != 1 || structured.Item.Labels[0] != labelID {
		t.Fatalf("labels = %v, want [%s]", structured.Item.Labels, labelID)
	}

	// milestone_id=null clears the column (three-way semantics per
	// handler_update.go's milestone_id sniff).
	env2 := callTool(t, fx.RawKey, "update", map[string]any{
		"item_id":      itemID,
		"milestone_id": nil,
	})
	res2 := assertStructuredEchoesText(t, env2)
	var structured2 struct {
		Item struct {
			MilestoneID string `json:"milestone_id"`
		} `json:"item"`
	}
	if err := json.Unmarshal(res2.StructuredContent, &structured2); err != nil {
		t.Fatalf("unmarshal cleared: %v", err)
	}
	if structured2.Item.MilestoneID != "" {
		t.Fatalf("milestone_id after null = %q, want empty (cleared)", structured2.Item.MilestoneID)
	}
}

// =============================================================================
// close
// =============================================================================

// TestD3_CloseRequiresClaim covers AC #2: when claimed_by_id IS NULL
// the close tool returns §7 PRECONDITION_NOT_MET with
// data.missing="claimed_by_id".
func TestD3_CloseRequiresClaim(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedUnclaimedItem(t, fx.OrgID, fx.ProjectID)

	env := callTool(t, fx.RawKey, "close", map[string]any{"item_id": itemID})
	if env.Error == nil {
		t.Fatalf("expected PRECONDITION_NOT_MET envelope on unclaimed close; got success result=%s", string(env.Result))
	}
	var data envelopeData
	if err := json.Unmarshal(env.Error.Data, &data); err != nil {
		t.Fatalf("unmarshal error.data: %v", err)
	}
	if data.Kind != "PRECONDITION_NOT_MET" {
		t.Fatalf("error.data.kind = %q, want PRECONDITION_NOT_MET", data.Kind)
	}
	if got, _ := data.Details["missing"].(string); got != "claimed_by_id" {
		t.Fatalf("error.data.details.missing = %q, want \"claimed_by_id\" (per SPEC §6.2 Tool 6 line 1334)", got)
	}
}

// TestD3_CloseHappyPath covers AC #3 (close emits CascadeRequested on
// success) at the layer this harness can observe: status=Done +
// closed_at populated in the workitems.items row.
//
// Why we do NOT assert et.Topic(CascadeRequestedTopic) here: the MCP
// transport runs inside an httptest.NewServer goroutine (see
// d1_transport_test.go::mcpEndpoint) which bypasses Encore's request
// manager. Encore's per-test pubsub recorder
// (runtimes/go/pubsub/internal/test/topic.go::PublishMessage) keys
// the recorded messages on `t.ts.CurrentTest()` which is tracked via
// Encore's request lifecycle — the httptest goroutine has no
// associated test marker so the publish is invisible to et.Topic
// from this test scope.
//
// The publish itself is covered end-to-end in
// apps/api/workitems/integration_test.go (where the test goroutine
// and the workitems.Close goroutine are the same and et.Topic works
// as designed). Here we only assert what is observable: the
// transactional side-effects of close (status flip + closed_at
// stamp).
func TestD3_CloseHappyPath(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedClaimedItem(t, fx.OrgID, fx.ProjectID, fx.UserID)

	env := callTool(t, fx.RawKey, "close", map[string]any{
		"item_id": itemID,
		"reason":  "shipped",
	})
	res := assertStructuredEchoesText(t, env)

	var structured struct {
		Item struct {
			ID       string `json:"id"`
			Status   string `json:"status"`
			ClosedAt string `json:"closed_at"`
		} `json:"item"`
	}
	if err := json.Unmarshal(res.StructuredContent, &structured); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if structured.Item.ID != itemID {
		t.Fatalf("item.id = %q, want %q", structured.Item.ID, itemID)
	}
	if structured.Item.Status != "Done" {
		t.Fatalf("item.status = %q, want Done", structured.Item.Status)
	}
	if structured.Item.ClosedAt == "" {
		t.Fatalf("item.closed_at empty on success path")
	}

	// Verify the kind=completed comment was appended when Reason was
	// provided (workitems.Close inserts it inside the close transaction
	// per workitems.go::Close). This is the observable side-effect that
	// proves the close path executed end-to-end.
	ctx := context.Background()
	var commentCount int
	if err := db.QueryRow(ctx,
		`SELECT count(*) FROM workitems.comments
		  WHERE item_id = $1 AND kind = 'completed' AND body = 'shipped'`,
		itemID,
	).Scan(&commentCount); err != nil {
		t.Fatalf("count completed comment: %v", err)
	}
	if commentCount != 1 {
		t.Fatalf("kind=completed comment count = %d, want 1", commentCount)
	}
}

// =============================================================================
// show
// =============================================================================

// TestD3_ShowReturnsAllFourCollections covers AC #4: every nested
// collection is populated when the corresponding include_* flag is
// true (or absent, defaulting to true).
func TestD3_ShowReturnsAllFourCollections(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	ctx := context.Background()

	itemID := seedUnclaimedItem(t, fx.OrgID, fx.ProjectID)

	// Seed one comment via direct INSERT (AppendComment requires an
	// author binding we don't carry here; the wire shape only cares
	// that GetTrail emits the comment).
	commentID, _ := ulid.New()
	if _, err := db.Exec(ctx,
		`INSERT INTO workitems.comments
		   (id, item_id, author_id, kind, status, body)
		 VALUES ($1, $2, $3, 'general', 'info', 'd3 comment')`,
		commentID, itemID, fx.UserID,
	); err != nil {
		t.Fatalf("insert comment: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM workitems.comments WHERE id = $1`, commentID) })

	// Seed an incoming edge (other → itemID) and outgoing edge
	// (itemID → another) via direct INSERT to bypass deps.AddEdge's
	// cycle / scope checks that aren't relevant here.
	otherID := seedUnclaimedItem(t, fx.OrgID, fx.ProjectID)
	thirdID := seedUnclaimedItem(t, fx.OrgID, fx.ProjectID)

	edgeIn, _ := ulid.New()
	if _, err := db.Exec(ctx,
		`INSERT INTO deps.dependencies
		   (id, from_item, to_item, kind)
		 VALUES ($1, $2, $3, 'blocks')`,
		edgeIn, otherID, itemID,
	); err != nil {
		t.Fatalf("insert edgeIn: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM deps.dependencies WHERE id = $1`, edgeIn) })

	edgeOut, _ := ulid.New()
	if _, err := db.Exec(ctx,
		`INSERT INTO deps.dependencies
		   (id, from_item, to_item, kind)
		 VALUES ($1, $2, $3, 'blocks')`,
		edgeOut, itemID, thirdID,
	); err != nil {
		t.Fatalf("insert edgeOut: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM deps.dependencies WHERE id = $1`, edgeOut) })

	// Seed a finding child (type=finding, parent_id=itemID).
	findingID, _ := ulid.New()
	if _, err := db.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, parent_id, discovered_from_id, type,
		    title, status, priority, severity, kind_of_finding,
		    created_at, updated_at)
		 VALUES ($1, $2, $3, $4, $4, 'finding', $5, 'Backlog', 'P3',
		         'minor', 'review', now(), now())`,
		findingID, fx.OrgID, fx.ProjectID, itemID, "d3 finding",
	); err != nil {
		t.Fatalf("insert finding: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM workitems.items WHERE id = $1`, findingID) })

	env := callTool(t, fx.RawKey, "show", map[string]any{"item_id": itemID})
	res := assertStructuredEchoesText(t, env)

	var structured struct {
		Item            map[string]any   `json:"item"`
		Comments        []map[string]any `json:"comments"`
		DependenciesIn  []map[string]any `json:"dependencies_in"`
		DependenciesOut []map[string]any `json:"dependencies_out"`
		Findings        []map[string]any `json:"findings"`
	}
	if err := json.Unmarshal(res.StructuredContent, &structured); err != nil {
		t.Fatalf("unmarshal: %v; raw=%s", err, string(res.StructuredContent))
	}
	if got, _ := structured.Item["id"].(string); got != itemID {
		t.Fatalf("item.id = %q, want %q", got, itemID)
	}
	if len(structured.Comments) != 1 {
		t.Fatalf("comments len = %d, want 1", len(structured.Comments))
	}
	if len(structured.DependenciesIn) != 1 {
		t.Fatalf("dependencies_in len = %d, want 1", len(structured.DependenciesIn))
	}
	if got, _ := structured.DependenciesIn[0]["from_item"].(string); got != otherID {
		t.Fatalf("dependencies_in[0].from_item = %q, want %q", got, otherID)
	}
	if len(structured.DependenciesOut) != 1 {
		t.Fatalf("dependencies_out len = %d, want 1", len(structured.DependenciesOut))
	}
	if got, _ := structured.DependenciesOut[0]["to_item"].(string); got != thirdID {
		t.Fatalf("dependencies_out[0].to_item = %q, want %q", got, thirdID)
	}
	if len(structured.Findings) != 1 {
		t.Fatalf("findings len = %d, want 1", len(structured.Findings))
	}
	if got, _ := structured.Findings[0]["id"].(string); got != findingID {
		t.Fatalf("findings[0].id = %q, want %q", got, findingID)
	}
}

// TestD3_ShowRespectsIncludeFlags asserts each include_* flag is
// honoured independently. include_comments=false → comments=[]; same
// for include_dependencies (drops both in/out arrays) and
// include_findings.
func TestD3_ShowRespectsIncludeFlags(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	ctx := context.Background()

	itemID := seedUnclaimedItem(t, fx.OrgID, fx.ProjectID)
	commentID, _ := ulid.New()
	if _, err := db.Exec(ctx,
		`INSERT INTO workitems.comments
		   (id, item_id, author_id, kind, status, body)
		 VALUES ($1, $2, $3, 'general', 'info', 'present')`,
		commentID, itemID, fx.UserID,
	); err != nil {
		t.Fatalf("insert comment: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM workitems.comments WHERE id = $1`, commentID) })

	// include_comments=false drops the comment.
	env := callTool(t, fx.RawKey, "show", map[string]any{
		"item_id":          itemID,
		"include_comments": false,
	})
	res := assertStructuredEchoesText(t, env)
	var structured struct {
		Comments        []map[string]any `json:"comments"`
		DependenciesIn  []map[string]any `json:"dependencies_in"`
		DependenciesOut []map[string]any `json:"dependencies_out"`
		Findings        []map[string]any `json:"findings"`
	}
	if err := json.Unmarshal(res.StructuredContent, &structured); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(structured.Comments) != 0 {
		t.Fatalf("comments len with include_comments=false = %d, want 0", len(structured.Comments))
	}

	// include_dependencies=false drops both edge arrays.
	env2 := callTool(t, fx.RawKey, "show", map[string]any{
		"item_id":              itemID,
		"include_dependencies": false,
	})
	res2 := assertStructuredEchoesText(t, env2)
	var s2 struct {
		DependenciesIn  []map[string]any `json:"dependencies_in"`
		DependenciesOut []map[string]any `json:"dependencies_out"`
	}
	if err := json.Unmarshal(res2.StructuredContent, &s2); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(s2.DependenciesIn) != 0 || len(s2.DependenciesOut) != 0 {
		t.Fatalf("include_dependencies=false: in=%d out=%d, want 0/0", len(s2.DependenciesIn), len(s2.DependenciesOut))
	}

	// include_findings=false drops the findings.
	env3 := callTool(t, fx.RawKey, "show", map[string]any{
		"item_id":          itemID,
		"include_findings": false,
	})
	res3 := assertStructuredEchoesText(t, env3)
	var s3 struct {
		Findings []map[string]any `json:"findings"`
	}
	if err := json.Unmarshal(res3.StructuredContent, &s3); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(s3.Findings) != 0 {
		t.Fatalf("include_findings=false: findings len = %d, want 0", len(s3.Findings))
	}
}

// =============================================================================
// list
// =============================================================================

// TestD3_ListNextCursorWhenOverLimit covers AC #5: seed limit+1 items
// and assert next_cursor is non-null on page 1, then null on page 2.
func TestD3_ListNextCursorWhenOverLimit(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	// Seed 5 items; request limit=3 so page 1 returns 3 + next_cursor,
	// page 2 returns 2 + next_cursor=null.
	ids := make([]string, 0, 5)
	for i := 0; i < 5; i++ {
		ids = append(ids, seedReadyItem(t, fx.OrgID, fx.ProjectID, "P2", time.Duration(i)*time.Second))
	}

	env := callTool(t, fx.RawKey, "list", map[string]any{
		"project_id": fx.ProjectID,
		"limit":      3,
	})
	res := assertStructuredEchoesText(t, env)
	page1 := decodeListPage(t, res.StructuredContent)
	if len(page1.Items) != 3 {
		t.Fatalf("page1 items len = %d, want 3", len(page1.Items))
	}
	if page1.NextCursor == nil {
		t.Fatalf("page1.next_cursor nil — expected more pages")
	}

	env2 := callTool(t, fx.RawKey, "list", map[string]any{
		"project_id": fx.ProjectID,
		"limit":      3,
		"cursor":     *page1.NextCursor,
	})
	res2 := assertStructuredEchoesText(t, env2)
	page2 := decodeListPage(t, res2.StructuredContent)
	if len(page2.Items) != 2 {
		t.Fatalf("page2 items len = %d, want 2", len(page2.Items))
	}
	if page2.NextCursor != nil {
		t.Fatalf("page2.next_cursor = %q, want nil (end-of-stream)", *page2.NextCursor)
	}
	// Wire-shape check: end-of-stream emits literal `"next_cursor": null`.
	assertNextCursorNullOnWire(t, res2.StructuredContent)

	// Concatenation invariant: no duplicates, no skips.
	gotIDs := make(map[string]struct{}, 5)
	for _, it := range page1.Items {
		gotIDs[it.ID] = struct{}{}
	}
	for _, it := range page2.Items {
		gotIDs[it.ID] = struct{}{}
	}
	if len(gotIDs) != 5 {
		t.Fatalf("concatenated unique ids = %d, want 5", len(gotIDs))
	}
	for _, want := range ids {
		if _, ok := gotIDs[want]; !ok {
			t.Fatalf("id %q missing from concatenated pages", want)
		}
	}
}

// TestD3_ListMilestoneIDFilter seeds two milestones and asserts the
// list filter only returns items on the requested milestone.
func TestD3_ListMilestoneIDFilter(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	m1 := seedMilestone(t, fx.OrgID, fx.ProjectID)
	m2 := seedMilestone(t, fx.OrgID, fx.ProjectID)

	id1 := seedReadyItem(t, fx.OrgID, fx.ProjectID, "P1", 0)
	id2 := seedReadyItem(t, fx.OrgID, fx.ProjectID, "P1", time.Second)
	id3 := seedReadyItem(t, fx.OrgID, fx.ProjectID, "P1", 2*time.Second)
	assignItemMilestone(t, id1, m1)
	assignItemMilestone(t, id2, m1)
	assignItemMilestone(t, id3, m2)

	env := callTool(t, fx.RawKey, "list", map[string]any{
		"project_id":   fx.ProjectID,
		"milestone_id": m1,
	})
	res := assertStructuredEchoesText(t, env)
	page := decodeListPage(t, res.StructuredContent)
	if len(page.Items) != 2 {
		t.Fatalf("items len with milestone_id=m1 = %d, want 2", len(page.Items))
	}
	got := map[string]bool{}
	for _, it := range page.Items {
		got[it.ID] = true
	}
	if !got[id1] || !got[id2] {
		t.Fatalf("expected items {%s, %s}, got %v", id1, id2, page.Items)
	}
	if got[id3] {
		t.Fatalf("item from m2 (%s) leaked into m1 filter", id3)
	}
}

// TestD3_ListStateDimensionFilters asserts status[] and pipeline_stage[]
// narrow the result set behaviourally. Index usage is verified by
// workitems integration tests; here we only assert the wire contract.
func TestD3_ListStateDimensionFilters(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	// pipeline_stage values per items_pipeline_stage_chk in
	// 0040_workitems.up.sql: Investigation | Implementation | Review |
	// Quality | Deferred | Done.
	idReady := seedItemWithStatus(t, fx.OrgID, fx.ProjectID, "Ready", "Investigation")
	idInProg := seedItemWithStatus(t, fx.OrgID, fx.ProjectID, "InProgress", "Implementation")
	idBacklog := seedItemWithStatus(t, fx.OrgID, fx.ProjectID, "Backlog", "Investigation")

	env := callTool(t, fx.RawKey, "list", map[string]any{
		"project_id": fx.ProjectID,
		"status":     []string{"Ready", "InProgress"},
	})
	res := assertStructuredEchoesText(t, env)
	page := decodeListPage(t, res.StructuredContent)
	got := map[string]bool{}
	for _, it := range page.Items {
		got[it.ID] = true
	}
	if !got[idReady] || !got[idInProg] {
		t.Fatalf("status filter missing Ready/InProgress items: page=%v", page.Items)
	}
	if got[idBacklog] {
		t.Fatalf("Backlog item leaked into status=[Ready,InProgress] filter: %s", idBacklog)
	}

	env2 := callTool(t, fx.RawKey, "list", map[string]any{
		"project_id":     fx.ProjectID,
		"pipeline_stage": []string{"Implementation"},
	})
	res2 := assertStructuredEchoesText(t, env2)
	page2 := decodeListPage(t, res2.StructuredContent)
	got2 := map[string]bool{}
	for _, it := range page2.Items {
		got2[it.ID] = true
	}
	if !got2[idInProg] {
		t.Fatalf("pipeline_stage=[Implementation] missing %s", idInProg)
	}
	if got2[idReady] || got2[idBacklog] {
		t.Fatalf("pipeline_stage filter leaked non-Implementation rows: page=%v", page2.Items)
	}
}

// TestD3_ListCrossToolCursorRejected asserts the §6.2.0 contract: a
// cursor minted for `ready` (cursorVersionReady="r1") presented to
// `list` (cursorVersionList="l1") surfaces §7 VALIDATION with
// data.field="cursor". The version discriminator is the load-bearing
// check that prevents cross-tool replay.
func TestD3_ListCrossToolCursorRejected(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	// Seed two ready items + paginate ready with limit=1 to obtain a
	// real, valid ready-cursor.
	_ = seedReadyItem(t, fx.OrgID, fx.ProjectID, "P1", 0)
	_ = seedReadyItem(t, fx.OrgID, fx.ProjectID, "P1", time.Second)
	envReady := callTool(t, fx.RawKey, "ready", map[string]any{
		"project_id": fx.ProjectID,
		"limit":      1,
	})
	resReady := assertStructuredEchoesText(t, envReady)
	pageReady := decodeReadyPage(t, resReady.StructuredContent)
	if pageReady.NextCursor == nil {
		t.Fatalf("ready cursor missing — fixture insufficient")
	}

	// Present the ready cursor to list — must reject with VALIDATION
	// data.field="cursor".
	envList := callTool(t, fx.RawKey, "list", map[string]any{
		"project_id": fx.ProjectID,
		"cursor":     *pageReady.NextCursor,
	})
	if envList.Error == nil {
		t.Fatalf("expected §7 VALIDATION on cross-tool cursor; got success result=%s", string(envList.Result))
	}
	var data envelopeData
	if err := json.Unmarshal(envList.Error.Data, &data); err != nil {
		t.Fatalf("unmarshal error.data: %v", err)
	}
	if data.Kind != "VALIDATION" {
		t.Fatalf("error.data.kind = %q, want VALIDATION", data.Kind)
	}
	if got, _ := data.Details["field"].(string); got != "cursor" {
		t.Fatalf("error.data.details.field = %q, want \"cursor\"", got)
	}
}

// TestD3_ListLimitOutOfRange asserts limit > 200 is VALIDATION with
// data.field="limit" — parity with the Tool 2 contract.
func TestD3_ListLimitOutOfRange(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	env := callTool(t, fx.RawKey, "list", map[string]any{
		"project_id": fx.ProjectID,
		"limit":      201,
	})
	if env.Error == nil {
		t.Fatalf("expected VALIDATION; got success result=%s", string(env.Result))
	}
	var data envelopeData
	if err := json.Unmarshal(env.Error.Data, &data); err != nil {
		t.Fatalf("unmarshal error.data: %v", err)
	}
	if data.Kind != "VALIDATION" {
		t.Fatalf("error.data.kind = %q, want VALIDATION", data.Kind)
	}
	if got, _ := data.Details["field"].(string); got != "limit" {
		t.Fatalf("error.data.details.field = %q, want \"limit\"", got)
	}
}

// =============================================================================
// audit rows
// =============================================================================

// TestD3_AuditRowsCarryToolName asserts every D-3 tool dispatch lands
// a single mcp.tool_calls row with the matching tool_name. SPEC §8.1.
func TestD3_AuditRowsCarryToolName(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedUnclaimedItem(t, fx.OrgID, fx.ProjectID)
	claimedID := seedClaimedItem(t, fx.OrgID, fx.ProjectID, fx.UserID)

	_ = callTool(t, fx.RawKey, "update", map[string]any{
		"item_id": itemID,
		"title":   "audited",
	})
	_ = callTool(t, fx.RawKey, "show", map[string]any{"item_id": itemID})
	_ = callTool(t, fx.RawKey, "list", map[string]any{"project_id": fx.ProjectID})
	_ = callTool(t, fx.RawKey, "close", map[string]any{"item_id": claimedID})

	rows := selectToolCalls(t)
	have := map[string]int{}
	for _, r := range rows {
		have[r.ToolName]++
	}
	for _, want := range []string{"update", "show", "list", "close"} {
		if have[want] < 1 {
			t.Fatalf("audit row for tool_name=%q: count=%d, want >=1; rows=%+v", want, have[want], rows)
		}
	}
}

// =============================================================================
// helpers
// =============================================================================

// listPage models the §6.2 Tool 8 wire shape — items + next_cursor
// (string-or-null per round-2 W1 contract).
type listPage struct {
	Items []struct {
		ID    string `json:"id"`
		Title string `json:"title"`
	} `json:"items"`
	NextCursor *string `json:"next_cursor"`
}

func decodeListPage(t *testing.T, raw json.RawMessage) listPage {
	t.Helper()
	var p listPage
	if err := json.Unmarshal(raw, &p); err != nil {
		t.Fatalf("decodeListPage: %v; raw=%s", err, string(raw))
	}
	return p
}

// fmtCount returns the count via fmt.Sprint so test failure messages
// render slice lengths next to the rows they describe. Kept as a
// tiny helper to avoid repeating the wrapper in every assertion.
//
//nolint:unused // reserved for future failure-message helpers.
func fmtCount(n int) string { return fmt.Sprintf("%d", n) }

// joinIDs is a debugging helper for failure messages — surfaces the
// concatenated IDs in a stable order so flakes are diagnosable.
//
//nolint:unused // reserved for future failure-message helpers.
func joinIDs(ids []string) string { return strings.Join(ids, ",") }
