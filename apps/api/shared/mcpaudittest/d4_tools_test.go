// d4_tools_test.go covers the D-4 (unblock-tv8.19) acceptance matrix
// for MCP tools 9 (`search`) and 10 (`comment`).
//
// Reuses the d2Fixture / callTool / assertStructuredEchoesText
// harness — same package, full Bearer-auth roundtrip through
// MCPHandler. Each test seeds an isolated org+user+project+api_key
// tuple so the §7 envelopes reach the wire with real Identity
// propagation through withIdentity.
//
// Coverage matrix (bead unblock-tv8.19 AC):
//
//   - search_ReturnsItemAndCommentHits: seed an item + a comment
//     hitting the same FTS term; assert one hit per source; source=
//     "item" has comment_id=null and source="comment" has a non-null
//     ULID; snippet ≤ 200 chars; covers AC #1 + #2 (UNION ALL plan).
//   - search_NextCursorPaginatesDeterministically: seed limit+1 hits
//     (all rows hit the same term so rank ties force the (item_id,
//     comment_id) tiebreakers); page 1 carries non-null next_cursor,
//     page 2 emits null; zero duplicates and zero skips on
//     concatenation; cross-tool cursor (Tool 2 ready) rejected with
//     §7 VALIDATION data.field="cursor" — covers §6.2.0 contract.
//   - search_LimitOutOfRange: limit > 100 returns VALIDATION
//     data.field="limit"; parity with Tools 2 and 8.
//   - search_RequiresQuery: empty/whitespace query returns VALIDATION
//     data.field="query" — symmetric with the JSON-schema description.
//   - comment_AppendValidatesEnums: table-driven across §6.5 kind +
//     status enums; happy path persists the row + structuredContent
//     mirrors content[0].text; unknown kind / unknown status / empty
//     body each return VALIDATION with the correct data.field.
//     Covers AC #3.
//   - comment_BodyExceedsCap: body length > 16384 returns VALIDATION
//     data.field="body" at the wire boundary (SPEC §6.2 Tool 10 lines
//     1474-1475, round-8 boundary clarification).
//   - comment_NoUpdateOrDeleteTool: tools/list response does NOT
//     include `update_comment` or `delete_comment`; append-only by
//     construction. Covers AC #4.
//   - d4_AuditRowsCarryToolName: search + comment dispatches each
//     write one mcp.tool_calls row with the matching tool_name.

package mcpaudittest

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"strings"
	"testing"
	"time"

	"encore.app/shared/ulid"
)

// seedSearchableItem inserts a workitems.items row with a body that
// contains `term` so the FTS index is hit deterministically. Returns
// the ULID. The created_at offset stays at NOW so the SQL plan is
// unaffected — search ordering is rank-based, not time-based.
func seedSearchableItem(t *testing.T, orgID, projectID, term string) string {
	t.Helper()
	ctx := context.Background()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, type, title, body, status, priority, created_at, updated_at)
		 VALUES ($1, $2, $3, 'task', $4, $5, 'Ready', 'P2', now(), now())`,
		id, orgID, projectID,
		"d4-search-"+term, "body containing "+term+" and more text",
	); err != nil {
		t.Fatalf("insert searchable item: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM workitems.items WHERE id = $1`, id) })
	return id
}

// seedSearchableComment inserts a workitems.comments row attached to
// an existing item with a body containing `term`. The author_id is
// populated so the AppendComment-style row passes the (author_id OR
// author_agent) CHECK invariant. Returns the comment ULID.
func seedSearchableComment(t *testing.T, itemID, authorID, term string) string {
	t.Helper()
	ctx := context.Background()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO workitems.comments
		   (id, item_id, author_id, kind, status, body)
		 VALUES ($1, $2, $3, 'general', 'info', $4)`,
		id, itemID, authorID, "comment about "+term+" for d4 search",
	); err != nil {
		t.Fatalf("insert searchable comment: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM workitems.comments WHERE id = $1`, id) })
	return id
}

// =============================================================================
// search
// =============================================================================

// TestD4_SearchReturnsItemAndCommentHits covers AC #1 + #2: the
// UNION ALL plan over items_fts_idx + comments_fts_idx returns one
// hit per source against the same term. source="item" carries
// comment_id=null; source="comment" carries a non-null ULID; snippet
// length stays under the 200-char cap.
func TestD4_SearchReturnsItemAndCommentHits(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	const term = "zinwald"
	itemID := seedSearchableItem(t, fx.OrgID, fx.ProjectID, term)
	commentID := seedSearchableComment(t, itemID, fx.UserID, term)

	env := callTool(t, fx.RawKey, "search", map[string]any{
		"project_id": fx.ProjectID,
		"query":      term,
	})
	res := assertStructuredEchoesText(t, env)
	page := decodeSearchPage(t, res.StructuredContent)

	if len(page.Hits) < 2 {
		t.Fatalf("hits len = %d, want >= 2; page=%+v", len(page.Hits), page.Hits)
	}
	if page.NextCursor != nil {
		t.Fatalf("next_cursor = %q, want nil on single-page result", *page.NextCursor)
	}

	var sawItem, sawComment bool
	for _, h := range page.Hits {
		if h.ItemID != itemID {
			t.Fatalf("hit.item_id = %q, want %q", h.ItemID, itemID)
		}
		if len(h.Snippet) > 200 {
			t.Fatalf("snippet length = %d, want <= 200", len(h.Snippet))
		}
		switch h.Source {
		case "item":
			sawItem = true
			if h.CommentID != nil {
				t.Fatalf("source=item must carry comment_id=null, got %q", *h.CommentID)
			}
		case "comment":
			sawComment = true
			if h.CommentID == nil || *h.CommentID != commentID {
				got := "<nil>"
				if h.CommentID != nil {
					got = *h.CommentID
				}
				t.Fatalf("source=comment comment_id = %s, want %s", got, commentID)
			}
		default:
			t.Fatalf("unknown source %q", h.Source)
		}
	}
	if !sawItem || !sawComment {
		t.Fatalf("expected UNION ALL hits across both sources; sawItem=%v sawComment=%v", sawItem, sawComment)
	}
}

// TestD4_SearchNextCursorPaginatesDeterministically: seed 3 items +
// 2 comments hitting the same term (5 hits total) and paginate with
// limit=2. Pages 1+2 each return 2 hits with non-null next_cursor;
// page 3 returns 1 hit with next_cursor=null. Concatenated, no
// duplicates and no skips. The rank-tie scenario (all rows match the
// same term) is the canonical FTS-cursor stress test — keyset
// tiebreakers (item_id, comment_id) must resolve deterministically.
func TestD4_SearchNextCursorPaginatesDeterministically(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	const term = "paginated"
	ids := make([]string, 0, 5)
	// 3 items, all matching the term.
	for i := 0; i < 3; i++ {
		ids = append(ids, seedSearchableItem(t, fx.OrgID, fx.ProjectID, term))
	}
	// 2 comments attached to the first item.
	cIDs := make([]string, 0, 2)
	for i := 0; i < 2; i++ {
		cIDs = append(cIDs, seedSearchableComment(t, ids[0], fx.UserID, term))
	}

	// Page 1.
	env1 := callTool(t, fx.RawKey, "search", map[string]any{
		"project_id": fx.ProjectID,
		"query":      term,
		"limit":      2,
	})
	res1 := assertStructuredEchoesText(t, env1)
	page1 := decodeSearchPage(t, res1.StructuredContent)
	if len(page1.Hits) != 2 {
		t.Fatalf("page1 hits = %d, want 2", len(page1.Hits))
	}
	if page1.NextCursor == nil {
		t.Fatalf("page1.next_cursor nil — expected more pages")
	}

	// Page 2.
	env2 := callTool(t, fx.RawKey, "search", map[string]any{
		"project_id": fx.ProjectID,
		"query":      term,
		"limit":      2,
		"cursor":     *page1.NextCursor,
	})
	res2 := assertStructuredEchoesText(t, env2)
	page2 := decodeSearchPage(t, res2.StructuredContent)
	if len(page2.Hits) != 2 {
		t.Fatalf("page2 hits = %d, want 2", len(page2.Hits))
	}
	if page2.NextCursor == nil {
		t.Fatalf("page2.next_cursor nil — expected one more page")
	}

	// Page 3.
	env3 := callTool(t, fx.RawKey, "search", map[string]any{
		"project_id": fx.ProjectID,
		"query":      term,
		"limit":      2,
		"cursor":     *page2.NextCursor,
	})
	res3 := assertStructuredEchoesText(t, env3)
	page3 := decodeSearchPage(t, res3.StructuredContent)
	if len(page3.Hits) != 1 {
		t.Fatalf("page3 hits = %d, want 1", len(page3.Hits))
	}
	if page3.NextCursor != nil {
		t.Fatalf("page3.next_cursor = %q, want nil (end-of-stream)", *page3.NextCursor)
	}
	// Wire-shape check: end-of-stream emits literal `"next_cursor": null`.
	assertNextCursorNullOnWire(t, res3.StructuredContent)

	// Concatenation invariant: 5 unique row identities (3 items +
	// 2 comments) across the three pages.
	seen := map[string]struct{}{}
	collect := func(p searchPage) {
		for _, h := range p.Hits {
			key := h.ItemID + "|"
			if h.CommentID != nil {
				key = h.ItemID + "|" + *h.CommentID
			}
			if _, dup := seen[key]; dup {
				t.Fatalf("duplicate hit across pages: %s", key)
			}
			seen[key] = struct{}{}
		}
	}
	collect(page1)
	collect(page2)
	collect(page3)
	if len(seen) != 5 {
		t.Fatalf("concatenated unique hits = %d, want 5", len(seen))
	}
	for _, want := range ids {
		if _, ok := seen[want+"|"]; !ok {
			t.Fatalf("item hit %q missing from concatenated pages", want)
		}
	}
	for _, want := range cIDs {
		if _, ok := seen[ids[0]+"|"+want]; !ok {
			t.Fatalf("comment hit %q missing from concatenated pages", want)
		}
	}
}

// TestD4_SearchCrossToolCursorRejected: a cursor minted for `ready`
// presented to `search` surfaces §7 VALIDATION with data.field=
// "cursor". The cursorVersionSearch="s1" discriminator is the
// load-bearing check that prevents cross-tool replay.
func TestD4_SearchCrossToolCursorRejected(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	// Mint a real ready cursor (need at least 2 ready items + limit=1).
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

	// Seed at least one searchable row so the search path is not
	// short-circuited before the cursor decode runs.
	_ = seedSearchableItem(t, fx.OrgID, fx.ProjectID, "ortholog")

	envSearch := callTool(t, fx.RawKey, "search", map[string]any{
		"project_id": fx.ProjectID,
		"query":      "ortholog",
		"cursor":     *pageReady.NextCursor,
	})
	if envSearch.Error == nil {
		t.Fatalf("expected §7 VALIDATION on cross-tool cursor; got success result=%s", string(envSearch.Result))
	}
	var data envelopeData
	if err := json.Unmarshal(envSearch.Error.Data, &data); err != nil {
		t.Fatalf("unmarshal error.data: %v", err)
	}
	if data.Kind != "VALIDATION" {
		t.Fatalf("error.data.kind = %q, want VALIDATION", data.Kind)
	}
	if got, _ := data.Details["field"].(string); got != "cursor" {
		t.Fatalf("error.data.details.field = %q, want \"cursor\"", got)
	}
}

// TestD4_SearchLimitOutOfRange: limit > 100 surfaces VALIDATION with
// data.field="limit". Parity with Tool 2 (1..200) and Tool 8 (1..200)
// at the appropriate ceiling for Tool 9 (1..100 per SPEC §6.2 Tool 9
// line 1428).
func TestD4_SearchLimitOutOfRange(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	env := callTool(t, fx.RawKey, "search", map[string]any{
		"project_id": fx.ProjectID,
		"query":      "anything",
		"limit":      101,
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

// TestD4_SearchRequiresQuery: empty / whitespace query returns
// VALIDATION data.field="query". The wire contract documents `query`
// as required; the handler enforces the boundary so the §6.2 Tool 9
// JSON-schema description is symmetric with runtime behaviour.
func TestD4_SearchRequiresQuery(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	for _, q := range []string{"", "   ", "\t\n"} {
		env := callTool(t, fx.RawKey, "search", map[string]any{
			"project_id": fx.ProjectID,
			"query":      q,
		})
		if env.Error == nil {
			t.Fatalf("query=%q expected VALIDATION; got success result=%s", q, string(env.Result))
		}
		var data envelopeData
		if err := json.Unmarshal(env.Error.Data, &data); err != nil {
			t.Fatalf("unmarshal error.data: %v", err)
		}
		if data.Kind != "VALIDATION" {
			t.Fatalf("query=%q error.data.kind = %q, want VALIDATION", q, data.Kind)
		}
		if got, _ := data.Details["field"].(string); got != "query" {
			t.Fatalf("query=%q error.data.details.field = %q, want \"query\"", q, got)
		}
	}
}

// =============================================================================
// comment
// =============================================================================

// TestD4_CommentAppendHappyPath asserts the §6.2 Tool 10 happy path:
// kind=general status=info body=<text> appends a row, the
// structuredContent mirrors content[0].text, and the persisted row
// carries author_id=identity.UserID + author_agent=identity.AgentKind
// (claude-code per the fixture).
func TestD4_CommentAppendHappyPath(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedUnclaimedItem(t, fx.OrgID, fx.ProjectID)

	env := callTool(t, fx.RawKey, "comment", map[string]any{
		"item_id": itemID,
		"kind":    "general",
		"status":  "info",
		"body":    "first d4 comment body",
	})
	res := assertStructuredEchoesText(t, env)
	got := decodeCommentOut(t, res.StructuredContent)

	if got.Comment.ItemID != itemID {
		t.Fatalf("comment.item_id = %q, want %q", got.Comment.ItemID, itemID)
	}
	if got.Comment.Kind != "general" {
		t.Fatalf("comment.kind = %q, want general", got.Comment.Kind)
	}
	if got.Comment.Status != "info" {
		t.Fatalf("comment.status = %q, want info", got.Comment.Status)
	}
	if got.Comment.AuthorID != fx.UserID {
		t.Fatalf("comment.author_id = %q, want %q", got.Comment.AuthorID, fx.UserID)
	}
	if got.Comment.AuthorAgent != "claude-code" {
		t.Fatalf("comment.author_agent = %q, want claude-code", got.Comment.AuthorAgent)
	}
	if got.Comment.Body != "first d4 comment body" {
		t.Fatalf("comment.body = %q, want body roundtrip", got.Comment.Body)
	}
	if got.Comment.ID == "" {
		t.Fatalf("comment.id empty")
	}
	if got.Comment.CreatedAt == "" {
		t.Fatalf("comment.created_at empty")
	}

	// DB read-back: row landed.
	ctx := context.Background()
	var dbID string
	if err := db.QueryRow(ctx, `SELECT id FROM workitems.comments WHERE id = $1`, got.Comment.ID).Scan(&dbID); err != nil {
		t.Fatalf("comment not persisted: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM workitems.comments WHERE id = $1`, got.Comment.ID) })
}

// TestD4_CommentValidatesEnums covers AC #3: unknown kind / unknown
// status / empty body each surface §7 VALIDATION with the matching
// data.field. PRD §6.5 kind+status enums are enforced by
// workitems.AppendComment via Meta.field; errmap projects to §7
// data.field without changes.
func TestD4_CommentValidatesEnums(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedUnclaimedItem(t, fx.OrgID, fx.ProjectID)

	cases := []struct {
		name      string
		args      map[string]any
		wantField string
	}{
		{
			name: "unknown kind",
			args: map[string]any{
				"item_id": itemID,
				"kind":    "not-a-real-kind",
				"status":  "info",
				"body":    "body",
			},
			wantField: "kind",
		},
		{
			name: "unknown status",
			args: map[string]any{
				"item_id": itemID,
				"kind":    "general",
				"status":  "not-a-real-status",
				"body":    "body",
			},
			wantField: "status",
		},
		{
			name: "empty body",
			args: map[string]any{
				"item_id": itemID,
				"kind":    "general",
				"status":  "info",
				"body":    "",
			},
			wantField: "body",
		},
		{
			name: "whitespace body",
			args: map[string]any{
				"item_id": itemID,
				"kind":    "general",
				"status":  "info",
				"body":    "   \t  ",
			},
			wantField: "body",
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			env := callTool(t, fx.RawKey, "comment", tc.args)
			if env.Error == nil {
				t.Fatalf("%s: expected VALIDATION; got success result=%s", tc.name, string(env.Result))
			}
			var data envelopeData
			if err := json.Unmarshal(env.Error.Data, &data); err != nil {
				t.Fatalf("%s: unmarshal error.data: %v", tc.name, err)
			}
			if data.Kind != "VALIDATION" {
				t.Fatalf("%s: error.data.kind = %q, want VALIDATION", tc.name, data.Kind)
			}
			if got, _ := data.Details["field"].(string); got != tc.wantField {
				t.Fatalf("%s: error.data.details.field = %q, want %q", tc.name, got, tc.wantField)
			}
		})
	}
}

// TestD4_CommentBodyExceedsCap: body length > 16384 chars surfaces
// VALIDATION data.field="body" at the wire boundary (SPEC §6.2 Tool 10
// lines 1474-1475 — round-8 boundary clarification: handler enforces
// 1..16384, AppendComment enforces only the non-empty floor).
func TestD4_CommentBodyExceedsCap(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedUnclaimedItem(t, fx.OrgID, fx.ProjectID)

	body := strings.Repeat("x", 16385)
	env := callTool(t, fx.RawKey, "comment", map[string]any{
		"item_id": itemID,
		"kind":    "general",
		"status":  "info",
		"body":    body,
	})
	if env.Error == nil {
		t.Fatalf("expected VALIDATION on oversized body; got success result=%s", string(env.Result))
	}
	var data envelopeData
	if err := json.Unmarshal(env.Error.Data, &data); err != nil {
		t.Fatalf("unmarshal error.data: %v", err)
	}
	if data.Kind != "VALIDATION" {
		t.Fatalf("error.data.kind = %q, want VALIDATION", data.Kind)
	}
	if got, _ := data.Details["field"].(string); got != "body" {
		t.Fatalf("error.data.details.field = %q, want \"body\"", got)
	}

	// Exactly 16384 chars must pass (boundary check).
	bodyOK := strings.Repeat("y", 16384)
	envOK := callTool(t, fx.RawKey, "comment", map[string]any{
		"item_id": itemID,
		"kind":    "general",
		"status":  "info",
		"body":    bodyOK,
	})
	if envOK.Error != nil {
		t.Fatalf("body=16384 chars must pass; got error data=%s", string(envOK.Error.Data))
	}
	resOK := assertStructuredEchoesText(t, envOK)
	cmt := decodeCommentOut(t, resOK.StructuredContent)
	if cmt.Comment.ID != "" {
		ctx := context.Background()
		t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM workitems.comments WHERE id = $1`, cmt.Comment.ID) })
	}
}

// TestD4_CommentMissingItemID: missing item_id is rejected by the
// MCP SDK's JSON-schema pre-validation (item_id is declared without
// `omitempty` so the SDK marks it required at the schema layer). The
// rejection surfaces as a successful JSON-RPC `result` envelope with
// `isError:true` and a `content[0].text` carrying the SDK validator's
// message — distinct from the §7 VALIDATION envelope which only fires
// AFTER schema validation passes. We assert the SDK error path here
// because the wire-level guarantee is "missing item_id never executes
// AppendComment", which the SDK enforces upstream of our handler.
func TestD4_CommentMissingItemID(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	env := callTool(t, fx.RawKey, "comment", map[string]any{
		"kind":   "general",
		"status": "info",
		"body":   "body",
	})
	if env.Error != nil {
		t.Fatalf("expected SDK-level isError, got JSON-RPC error envelope data=%s", string(env.Error.Data))
	}
	var res toolCallResult
	if err := json.Unmarshal(env.Result, &res); err != nil {
		t.Fatalf("unmarshal result: %v", err)
	}
	if !res.IsError {
		t.Fatalf("missing item_id must produce isError=true result; got %+v", res)
	}
	if len(res.Content) == 0 || !strings.Contains(res.Content[0].Text, "item_id") {
		t.Fatalf("expected error text to mention item_id; got content=%+v", res.Content)
	}

	// Defense-in-depth: an EMPTY item_id (present but zero) bypasses
	// the SDK's required-field check (the field IS present) and reaches
	// the handler's own guard, which surfaces §7 VALIDATION with
	// data.field="item_id".
	env2 := callTool(t, fx.RawKey, "comment", map[string]any{
		"item_id": "",
		"kind":    "general",
		"status":  "info",
		"body":    "body",
	})
	if env2.Error == nil {
		t.Fatalf("expected §7 VALIDATION on empty item_id; got success result=%s", string(env2.Result))
	}
	var data envelopeData
	if err := json.Unmarshal(env2.Error.Data, &data); err != nil {
		t.Fatalf("unmarshal error.data: %v", err)
	}
	if data.Kind != "VALIDATION" {
		t.Fatalf("error.data.kind = %q, want VALIDATION", data.Kind)
	}
	if got, _ := data.Details["field"].(string); got != "item_id" {
		t.Fatalf("error.data.details.field = %q, want \"item_id\"", got)
	}
}

// TestD4_CommentNoUpdateOrDeleteTool covers AC #4: tools/list MUST NOT
// expose update_comment or delete_comment. Append-only by construction
// — P01 does NOT ship update/delete for comments (SPEC §6.2 Tool 10
// line 1472).
func TestD4_CommentNoUpdateOrDeleteTool(t *testing.T) {
	fx := seedD2Fixture(t)
	names := listToolNames(t, fx.RawKey)

	for _, banned := range []string{"update_comment", "delete_comment"} {
		for _, got := range names {
			if got == banned {
				t.Fatalf("tools/list exposes %q — append-only contract violated; names=%v", banned, names)
			}
		}
	}
	// Sanity: both d4 tools ARE present.
	for _, want := range []string{"search", "comment"} {
		var ok bool
		for _, got := range names {
			if got == want {
				ok = true
				break
			}
		}
		if !ok {
			t.Fatalf("tools/list missing %q; names=%v", want, names)
		}
	}
}

// =============================================================================
// audit rows
// =============================================================================

// TestD4_AuditRowsCarryToolName: each search + comment dispatch
// writes one mcp.tool_calls row with the matching tool_name. SPEC
// §8.1 — completes the audit coverage matrix alongside D-2 and D-3.
func TestD4_AuditRowsCarryToolName(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedUnclaimedItem(t, fx.OrgID, fx.ProjectID)
	_ = seedSearchableItem(t, fx.OrgID, fx.ProjectID, "auditterm")

	_ = callTool(t, fx.RawKey, "search", map[string]any{
		"project_id": fx.ProjectID,
		"query":      "auditterm",
	})
	_ = callTool(t, fx.RawKey, "comment", map[string]any{
		"item_id": itemID,
		"kind":    "general",
		"status":  "info",
		"body":    "audit-row body",
	})

	rows := selectToolCalls(t, fx.OrgID)
	have := map[string]int{}
	for _, r := range rows {
		have[r.ToolName]++
	}
	for _, want := range []string{"search", "comment"} {
		if have[want] < 1 {
			t.Fatalf("audit row for tool_name=%q: count=%d, want >=1; rows=%+v", want, have[want], rows)
		}
	}
}

// =============================================================================
// helpers
// =============================================================================

// searchHitWire models the §6.2 Tool 9 wire shape for one hit.
// comment_id is *string so source="item" rows (null) and source=
// "comment" rows (ULID) round-trip correctly.
type searchHitWire struct {
	ItemID    string  `json:"item_id"`
	Source    string  `json:"source"`
	CommentID *string `json:"comment_id"`
	Rank      float64 `json:"rank"`
	Snippet   string  `json:"snippet"`
}

// searchPage models the §6.2 Tool 9 wire shape — hits[] +
// next_cursor (string-or-null per round-2 W1 contract).
type searchPage struct {
	Hits       []searchHitWire `json:"hits"`
	NextCursor *string         `json:"next_cursor"`
}

func decodeSearchPage(t *testing.T, raw json.RawMessage) searchPage {
	t.Helper()
	var p searchPage
	if err := json.Unmarshal(raw, &p); err != nil {
		t.Fatalf("decodeSearchPage: %v; raw=%s", err, string(raw))
	}
	return p
}

// commentWireOut models the §6.2 Tool 10 wire shape — the
// structuredContent JSON object carrying { "comment": Comment }.
type commentWireOut struct {
	Comment struct {
		ID          string `json:"id"`
		ItemID      string `json:"item_id"`
		ParentID    string `json:"parent_id,omitempty"`
		AuthorID    string `json:"author_id,omitempty"`
		AuthorAgent string `json:"author_agent,omitempty"`
		Kind        string `json:"kind"`
		Status      string `json:"status"`
		Body        string `json:"body"`
		CreatedAt   string `json:"created_at"`
		UpdatedAt   string `json:"updated_at"`
	} `json:"comment"`
}

func decodeCommentOut(t *testing.T, raw json.RawMessage) commentWireOut {
	t.Helper()
	var out commentWireOut
	if err := json.Unmarshal(raw, &out); err != nil {
		t.Fatalf("decodeCommentOut: %v; raw=%s", err, string(raw))
	}
	return out
}

// listToolNames calls the JSON-RPC `tools/list` method against the
// running MCP test server and returns the names of every registered
// tool. The Bearer auth + initialize handshake mirrors callTool so
// the wire path is identical to a real client.
func listToolNames(t *testing.T, rawKey string) []string {
	t.Helper()

	// Initialize first to mint a session id.
	postReq, err := http.NewRequest(http.MethodPost, mcpEndpoint(), strings.NewReader(mcpInitializeBody))
	if err != nil {
		t.Fatalf("http.NewRequest initialize: %v", err)
	}
	postReq.Header.Set("Content-Type", "application/json")
	postReq.Header.Set("Accept", "application/json, text/event-stream")
	postReq.Header.Set("Authorization", "Bearer "+rawKey)
	initResp := httpDo(t, postReq, 5*time.Second)
	sessionID := initResp.Header.Get("Mcp-Session-Id")
	_, _ = io.Copy(io.Discard, initResp.Body)
	_ = initResp.Body.Close()
	if sessionID == "" {
		t.Fatalf("initialize did not return Mcp-Session-Id")
	}

	body := `{"jsonrpc":"2.0","id":99,"method":"tools/list","params":{}}`
	req, err := http.NewRequest(http.MethodPost, mcpEndpoint(), strings.NewReader(body))
	if err != nil {
		t.Fatalf("http.NewRequest tools/list: %v", err)
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json, text/event-stream")
	req.Header.Set("Authorization", "Bearer "+rawKey)
	req.Header.Set("Mcp-Session-Id", sessionID)

	resp := httpDo(t, req, 10*time.Second)
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		b, _ := io.ReadAll(resp.Body)
		t.Fatalf("tools/list status=%d body=%s", resp.StatusCode, string(b))
	}
	raw, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatalf("read tools/list body: %v", err)
	}
	payload := raw
	if strings.HasPrefix(resp.Header.Get("Content-Type"), "text/event-stream") {
		payload = extractFirstSSEData(t, raw)
	}

	var env struct {
		Result struct {
			Tools []struct {
				Name string `json:"name"`
			} `json:"tools"`
		} `json:"result"`
	}
	if err := json.Unmarshal(payload, &env); err != nil {
		t.Fatalf("unmarshal tools/list: %v; body=%s", err, string(payload))
	}
	names := make([]string, 0, len(env.Result.Tools))
	for _, tl := range env.Result.Tools {
		names = append(names, tl.Name)
	}
	if len(names) == 0 {
		t.Fatalf("tools/list returned empty tool set; raw=%s", string(payload))
	}
	return names
}
