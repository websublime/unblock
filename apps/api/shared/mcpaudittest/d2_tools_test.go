// d2_tools_test.go covers the D-2 (unblock-tv8.17) acceptance matrix
// for the four MCP tool handlers — prime, ready, claim, create.
//
// The test fixture exercises a full Bearer-auth roundtrip through
// the MCP transport (httptest wrapping serveMCP). Each test seeds
// an isolated org+user+project+api_key tuple so the §7 envelopes
// reach the wire with real Identity propagation through
// withIdentity (the bridge between the raw-endpoint Bearer hot
// path and the private-RPC encoreauth.UserID() surface — SPEC §
// 4.3.1 / 4.3.2 / 4.3.3 + Sherlock RISK 1 on bead unblock-tv8.17).
//
// Coverage:
//
//   - prime: returns ready_summary + claimed_by_me + recent_cascade_events
//     + empty memory_hints; structuredContent + content[0].text both
//     populated.
//   - ready: items ordered by (priority asc, created_at asc, id asc);
//     priority_min filter respected; total_ready accurate.
//   - claim: success structuredContent { claimed: true, item };
//     loser path returns §7 ALREADY_CLAIMED with winner_user_id,
//     winner_agent, claimed_at.
//   - create: success path + cycle-detected error (the orchestrator
//     DECISION decision #1 atomicity refactor is exercised: a
//     phantom item would survive otherwise; we assert post-error
//     the workitems.items row count is unchanged).
//   - audit row: each successful tool call writes one mcp.tool_calls
//     row with tool_name in {prime, ready, claim, create}, never
//     "transport".

package mcpaudittest

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"testing"
	"time"

	"encore.app/auth"
	"encore.app/shared/ulid"
)

// d2Fixture is the per-test environment for the D-2 tool tests.
// Owns an org, a user (issued_to_user on the API key), a project,
// and a Bearer raw key — everything needed to drive a full MCP
// request through serveMCP and into workitems / deps.
type d2Fixture struct {
	OrgID     string
	UserID    string
	ProjectID string
	RawKey    string
}

func seedD2Fixture(t *testing.T) d2Fixture {
	t.Helper()
	ctx := context.Background()

	orgID, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid orgID: %v", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO org.organizations (id, slug, name) VALUES ($1, $2, $3)`,
		orgID, "d2-"+strings.ToLower(orgID[len(orgID)-8:]), "D-2 test org "+orgID,
	); err != nil {
		t.Fatalf("insert org: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM org.organizations WHERE id = $1`, orgID) })

	userID, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid userID: %v", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO auth.users (id, primary_provider, primary_provider_id, email, display_name)
		 VALUES ($1, 'github', $2, $3, $4)`,
		userID, "d2-"+userID[len(userID)-8:],
		strings.ToLower(userID[len(userID)-8:])+"@d2.local", "d2-user",
	); err != nil {
		t.Fatalf("insert user: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM auth.users WHERE id = $1`, userID) })

	projectID, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid projectID: %v", err)
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO org.projects (id, org_id, slug, name) VALUES ($1, $2, $3, $4)`,
		projectID, orgID, "p-"+projectID[len(projectID)-8:], "d2 project",
	); err != nil {
		t.Fatalf("insert project: %v", err)
	}

	// Mint an API key bound to the user via IssuedToUser so Identity.UserID
	// is populated (Identity.UserID="" would fail withIdentity's
	// missing-identity guard).
	resp, err := auth.IssueAPIKey(ctx, &auth.IssueAPIKeyRequest{
		OrgID:        orgID,
		IssuedToUser: userID,
		Label:        "d2-tools-test",
		AgentKind:    "claude-code",
		Scopes:       []string{},
	})
	if err != nil {
		t.Fatalf("IssueAPIKey: %v", err)
	}

	return d2Fixture{
		OrgID:     orgID,
		UserID:    userID,
		ProjectID: projectID,
		RawKey:    resp.RawKey,
	}
}

// seedReadyItem inserts a Ready, unclaimed task directly via SQL.
// Used for the ready/claim/prime read paths so we control the
// is_ready=true state without going through the C-2 cascade
// subscriber (which does not fire under `encore test`).
//
// priority is one of "P0".."P4"; createdAt offsets allow stable
// ordering assertions. Returns the ULID.
func seedReadyItem(t *testing.T, orgID, projectID, priority string, createdAtOffset time.Duration) string {
	t.Helper()
	ctx := context.Background()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	// SQL clamps NOW() with the requested offset for ordering tests;
	// negative offsets place the item further in the past.
	if _, err := db.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, type, title, status, priority, is_ready, created_at, updated_at)
		 VALUES ($1, $2, $3, 'task', $4, 'Ready', $5, true, now() + ($6 || ' microseconds')::interval, now() + ($6 || ' microseconds')::interval)`,
		id, orgID, projectID,
		"d2-test-"+priority,
		priority,
		fmt.Sprintf("%d", createdAtOffset.Microseconds()),
	); err != nil {
		t.Fatalf("insert ready item: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec(ctx, `DELETE FROM workitems.items WHERE id = $1`, id) })
	return id
}

// callTool drives a JSON-RPC tools/call against the MCP test server.
// Returns the parsed envelope. Asserts HTTP 200 + JSON-RPC 2.0;
// callers walk the result or error field themselves.
func callTool(t *testing.T, rawKey, toolName string, arguments any) jsonRPCEnvelope {
	t.Helper()

	// MCP spec requires an initialize handshake before any tools/call.
	// Each test runs a fresh initialize to obtain a Mcp-Session-Id and
	// then issues the tools/call against the same session.
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
	initResp.Body.Close()
	if sessionID == "" {
		t.Fatalf("initialize did not return Mcp-Session-Id")
	}

	argsRaw, err := json.Marshal(arguments)
	if err != nil {
		t.Fatalf("marshal arguments: %v", err)
	}
	rpcBody := fmt.Sprintf(`{
		"jsonrpc": "2.0",
		"id": 42,
		"method": "tools/call",
		"params": {
			"name": %q,
			"arguments": %s
		}
	}`, toolName, string(argsRaw))

	req, err := http.NewRequest(http.MethodPost, mcpEndpoint(), strings.NewReader(rpcBody))
	if err != nil {
		t.Fatalf("http.NewRequest tools/call: %v", err)
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json, text/event-stream")
	req.Header.Set("Authorization", "Bearer "+rawKey)
	req.Header.Set("Mcp-Session-Id", sessionID)

	resp := httpDo(t, req, 10*time.Second)
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("tools/call status = %d, want 200; body=%s", resp.StatusCode, string(body))
	}

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatalf("read body: %v", err)
	}
	// The SDK may emit application/json or text/event-stream depending
	// on Accept negotiation. Extract the payload uniformly.
	payload := body
	if strings.HasPrefix(resp.Header.Get("Content-Type"), "text/event-stream") {
		payload = extractFirstSSEData(t, body)
	}

	var env jsonRPCEnvelope
	if err := json.Unmarshal(payload, &env); err != nil {
		t.Fatalf("unmarshal envelope: %v; body=%s", err, string(payload))
	}
	return env
}

// jsonRPCEnvelope is the test-side shape of a tools/call response.
// We deliberately decode result/error as RawMessage so each test
// can pick the right sub-shape and validate it against the spec.
type jsonRPCEnvelope struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      any             `json:"id"`
	Result  json.RawMessage `json:"result,omitempty"`
	Error   *envelopeError  `json:"error,omitempty"`
}

type envelopeError struct {
	Code    int             `json:"code"`
	Message string          `json:"message"`
	Data    json.RawMessage `json:"data"`
}

type envelopeData struct {
	Kind    string         `json:"kind"`
	Tool    string         `json:"tool"`
	TraceID string         `json:"trace_id"`
	Details map[string]any `json:"details"`
}

// toolCallResult is the structured shape the SDK emits for a
// successful tool dispatch. Content[0].text MUST be a JSON string
// that round-trips to StructuredContent — that is the §6.1 framing
// invariant the tests assert per AC #5.
type toolCallResult struct {
	Content []struct {
		Type string `json:"type"`
		Text string `json:"text"`
	} `json:"content"`
	StructuredContent json.RawMessage `json:"structuredContent"`
	IsError           bool            `json:"isError"`
}

// assertStructuredEchoesText asserts that content[0].text is a JSON
// string whose canonical form matches structuredContent — i.e. SPEC
// §6.1 line 1137 ("replicates the JSON in content[0].text"). The
// canonical compare normalises whitespace.
func assertStructuredEchoesText(t *testing.T, env jsonRPCEnvelope) toolCallResult {
	t.Helper()
	if env.Error != nil {
		t.Fatalf("expected success, got error: code=%d message=%s data=%s", env.Error.Code, env.Error.Message, string(env.Error.Data))
	}
	var res toolCallResult
	if err := json.Unmarshal(env.Result, &res); err != nil {
		t.Fatalf("unmarshal result: %v; body=%s", err, string(env.Result))
	}
	if res.IsError {
		t.Fatalf("isError = true on success path; structured=%s", string(res.StructuredContent))
	}
	if len(res.Content) == 0 {
		t.Fatalf("content[] empty on success path")
	}
	if res.Content[0].Type != "text" {
		t.Fatalf("content[0].type = %q, want text", res.Content[0].Type)
	}
	// Canonical compare: re-marshal both sides through encoding/json so
	// whitespace and field order do not matter.
	var lhs, rhs any
	if err := json.Unmarshal([]byte(res.Content[0].Text), &lhs); err != nil {
		t.Fatalf("content[0].text is not valid JSON: %v; text=%s", err, res.Content[0].Text)
	}
	if err := json.Unmarshal(res.StructuredContent, &rhs); err != nil {
		t.Fatalf("structuredContent is not valid JSON: %v; raw=%s", err, string(res.StructuredContent))
	}
	lhsCanon, _ := json.Marshal(lhs)
	rhsCanon, _ := json.Marshal(rhs)
	if !bytes.Equal(lhsCanon, rhsCanon) {
		t.Fatalf("content[0].text does not match structuredContent:\n text:  %s\n struct:%s",
			string(lhsCanon), string(rhsCanon))
	}
	return res
}

// TestD2_PrimeReturnsFullDashboard exercises the §6.2 Tool 1 happy path:
// structuredContent.ready_summary populated with up to ready_limit
// items, claimed_by_me reflects items currently claimed by the caller,
// recent_cascade_events is present (may be empty), memory_hints is the
// empty array (P01).
func TestD2_PrimeReturnsFullDashboard(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	seedReadyItem(t, fx.OrgID, fx.ProjectID, "P1", 0)
	seedReadyItem(t, fx.OrgID, fx.ProjectID, "P2", time.Second)

	env := callTool(t, fx.RawKey, "prime", map[string]any{
		"project_id":  fx.ProjectID,
		"ready_limit": 10,
	})
	res := assertStructuredEchoesText(t, env)

	var structured struct {
		ReadySummary struct {
			CountTotal int `json:"count_total"`
			Items      []struct {
				ID       string `json:"id"`
				Priority string `json:"priority"`
			} `json:"items"`
		} `json:"ready_summary"`
		ClaimedByMe         json.RawMessage `json:"claimed_by_me"`
		RecentCascadeEvents json.RawMessage `json:"recent_cascade_events"`
		MemoryHints         []any           `json:"memory_hints"`
	}
	if err := json.Unmarshal(res.StructuredContent, &structured); err != nil {
		t.Fatalf("unmarshal structured: %v; raw=%s", err, string(res.StructuredContent))
	}
	if structured.ReadySummary.CountTotal != 2 {
		t.Fatalf("count_total = %d, want 2", structured.ReadySummary.CountTotal)
	}
	if len(structured.ReadySummary.Items) != 2 {
		t.Fatalf("ready_summary.items len = %d, want 2", len(structured.ReadySummary.Items))
	}
	// Spec: ORDER BY priority ASC, created_at ASC — P1 before P2.
	if structured.ReadySummary.Items[0].Priority != "P1" {
		t.Fatalf("ready_summary.items[0].priority = %q, want P1", structured.ReadySummary.Items[0].Priority)
	}
	if structured.MemoryHints == nil {
		t.Fatalf("memory_hints must be present (empty array allowed in P01)")
	}
	if len(structured.MemoryHints) != 0 {
		t.Fatalf("memory_hints len = %d, want 0 in P01", len(structured.MemoryHints))
	}

	// Audit row: one for the initialize handshake's authenticated POST
	// (tool_name="transport" baseline since the SDK's initialize does
	// not dispatch a tool), one for tools/call → tool_name="prime".
	rows := selectToolCalls(t)
	primeRows := 0
	for _, r := range rows {
		if r.ToolName == "prime" {
			primeRows++
		}
	}
	if primeRows != 1 {
		t.Fatalf("audit rows with tool_name=prime: %d, want 1 (rows=%+v)", primeRows, rows)
	}
}

// TestD2_ReadyOrderingDeterministic asserts the §6.2 Tool 2 ordering
// invariant: (priority asc, created_at asc, id asc). Seeds three
// items: P0, P2, P2. Expected order: P0 first, then the two P2 items
// in created_at order.
func TestD2_ReadyOrderingDeterministic(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	idP2Early := seedReadyItem(t, fx.OrgID, fx.ProjectID, "P2", -2*time.Second)
	_ = seedReadyItem(t, fx.OrgID, fx.ProjectID, "P0", -time.Second)
	idP2Late := seedReadyItem(t, fx.OrgID, fx.ProjectID, "P2", 0)

	env := callTool(t, fx.RawKey, "ready", map[string]any{
		"project_id": fx.ProjectID,
		"limit":      10,
	})
	res := assertStructuredEchoesText(t, env)

	var structured struct {
		Items []struct {
			ID       string `json:"id"`
			Priority string `json:"priority"`
		} `json:"items"`
		TotalReady int `json:"total_ready"`
	}
	if err := json.Unmarshal(res.StructuredContent, &structured); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if structured.TotalReady != 3 {
		t.Fatalf("total_ready = %d, want 3", structured.TotalReady)
	}
	if len(structured.Items) != 3 {
		t.Fatalf("items len = %d, want 3", len(structured.Items))
	}
	if structured.Items[0].Priority != "P0" {
		t.Fatalf("items[0].priority = %q, want P0", structured.Items[0].Priority)
	}
	if structured.Items[1].ID != idP2Early || structured.Items[1].Priority != "P2" {
		t.Fatalf("items[1] = {id=%q, priority=%q}, want {id=%q, priority=P2}",
			structured.Items[1].ID, structured.Items[1].Priority, idP2Early)
	}
	if structured.Items[2].ID != idP2Late {
		t.Fatalf("items[2].id = %q, want %q", structured.Items[2].ID, idP2Late)
	}
}

// TestD2_ReadyPriorityMinFilters exercises the priority_min argument:
// items with priority > priority_min are excluded.
func TestD2_ReadyPriorityMinFilters(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	seedReadyItem(t, fx.OrgID, fx.ProjectID, "P0", 0)
	seedReadyItem(t, fx.OrgID, fx.ProjectID, "P3", time.Second)

	env := callTool(t, fx.RawKey, "ready", map[string]any{
		"project_id":   fx.ProjectID,
		"priority_min": "P1", // only P0..P1 allowed; P3 excluded
	})
	res := assertStructuredEchoesText(t, env)

	var structured struct {
		Items      []map[string]any `json:"items"`
		TotalReady int              `json:"total_ready"`
	}
	if err := json.Unmarshal(res.StructuredContent, &structured); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if structured.TotalReady != 1 {
		t.Fatalf("total_ready = %d, want 1 (P3 excluded by priority_min=P1)", structured.TotalReady)
	}
	if got, _ := structured.Items[0]["priority"].(string); got != "P0" {
		t.Fatalf("items[0].priority = %q, want P0", got)
	}
}

// TestD2_ClaimHappyPath asserts the §6.2 Tool 3 success envelope.
func TestD2_ClaimHappyPath(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedReadyItem(t, fx.OrgID, fx.ProjectID, "P1", 0)

	env := callTool(t, fx.RawKey, "claim", map[string]any{"item_id": itemID})
	res := assertStructuredEchoesText(t, env)

	var structured struct {
		Claimed bool `json:"claimed"`
		Item    struct {
			ID            string `json:"id"`
			Status        string `json:"status"`
			ClaimedByID   string `json:"claimed_by_id"`
			ClaimedAt     string `json:"claimed_at"`
			ClaimedByAgnt string `json:"claimed_by_agent"`
		} `json:"item"`
	}
	if err := json.Unmarshal(res.StructuredContent, &structured); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if !structured.Claimed {
		t.Fatalf("claimed = false on success path")
	}
	if structured.Item.Status != "InProgress" {
		t.Fatalf("item.status = %q, want InProgress", structured.Item.Status)
	}
	if structured.Item.ClaimedByID != fx.UserID {
		t.Fatalf("item.claimed_by_id = %q, want %q", structured.Item.ClaimedByID, fx.UserID)
	}
	if structured.Item.ClaimedByAgnt != "claude-code" {
		t.Fatalf("item.claimed_by_agent = %q, want claude-code", structured.Item.ClaimedByAgnt)
	}
	if structured.Item.ClaimedAt == "" {
		t.Fatalf("item.claimed_at empty on success path")
	}
}

// TestD2_ClaimLoserPath asserts §6.2 Tool 3 ALREADY_CLAIMED envelope:
// data.kind = ALREADY_CLAIMED, data.details.{winner_user_id,
// winner_agent, claimed_at}.
func TestD2_ClaimLoserPath(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	itemID := seedReadyItem(t, fx.OrgID, fx.ProjectID, "P1", 0)

	// First claim wins.
	first := callTool(t, fx.RawKey, "claim", map[string]any{"item_id": itemID})
	assertStructuredEchoesText(t, first)

	// Second claim is the loser.
	loser := callTool(t, fx.RawKey, "claim", map[string]any{"item_id": itemID})
	if loser.Error == nil {
		t.Fatalf("expected §7 error envelope on loser path; got success result=%s", string(loser.Result))
	}
	if loser.Error.Code != -32000 {
		t.Fatalf("error.code = %d, want -32000", loser.Error.Code)
	}
	var data envelopeData
	if err := json.Unmarshal(loser.Error.Data, &data); err != nil {
		t.Fatalf("unmarshal error.data: %v", err)
	}
	if data.Kind != "ALREADY_CLAIMED" {
		t.Fatalf("error.data.kind = %q, want ALREADY_CLAIMED", data.Kind)
	}
	if data.Tool != "claim" {
		t.Fatalf("error.data.tool = %q, want claim", data.Tool)
	}
	if v, _ := data.Details["winner_user_id"].(string); v != fx.UserID {
		t.Fatalf("details.winner_user_id = %q, want %q", v, fx.UserID)
	}
	if v, _ := data.Details["winner_agent"].(string); v != "claude-code" {
		t.Fatalf("details.winner_agent = %q, want claude-code", v)
	}
	if v, _ := data.Details["claimed_at"].(string); v == "" {
		t.Fatalf("details.claimed_at empty")
	}
}

// TestD2_CreateHappyPath asserts §6.2 Tool 4 success: structuredContent
// .item carries the persisted row.
func TestD2_CreateHappyPath(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	env := callTool(t, fx.RawKey, "create", map[string]any{
		"project_id": fx.ProjectID,
		"type":       "task",
		"title":      "D-2 create happy path",
		"body":       "Created via MCP tool.",
		"priority":   "P2",
	})
	res := assertStructuredEchoesText(t, env)

	var structured struct {
		Item struct {
			ID       string `json:"id"`
			Title    string `json:"title"`
			Priority string `json:"priority"`
			Status   string `json:"status"`
		} `json:"item"`
	}
	if err := json.Unmarshal(res.StructuredContent, &structured); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(structured.Item.ID) != 26 {
		t.Fatalf("item.id len = %d, want 26 (ULID)", len(structured.Item.ID))
	}
	if structured.Item.Title != "D-2 create happy path" {
		t.Fatalf("item.title = %q", structured.Item.Title)
	}
	if structured.Item.Priority != "P2" {
		t.Fatalf("item.priority = %q, want P2", structured.Item.Priority)
	}
	if structured.Item.Status != "Backlog" {
		t.Fatalf("item.status = %q, want Backlog (default)", structured.Item.Status)
	}
}

// TestD2_CreateCycleDetectedAtomicity exercises the SPEC §6.2 Tool 4
// cycle path AND the orchestrator DECISION decision #1 atomicity
// refactor: when a dependency edge fails (here, a non-existent
// blocker_item_id triggering NOT_FOUND inside the same tx), the
// new item is NOT persisted.
//
// We use NOT_FOUND rather than a true cycle because building a real
// cycle scenario requires pre-creating items with incoming edges,
// which Tool 4 does not yet support (the bead orchestrator's
// mathematical note: Tool 4's new item has only incoming edges so
// cycles are impossible by construction). The atomicity property
// we are testing is "ANY downstream failure (FK, cycle, validation)
// rolls back the item insert" — the NOT_FOUND case exercises the
// same rollback path the cycle case would use in P02 once Tool 11
// (add_dependency) lets agents chain pre-existing items into a
// cycle that Tool 4 then closes.
func TestD2_CreateCycleDetectedAtomicity(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)

	// Bogus blocker_item_id → deps.AddEdgeInTx returns NotFound on
	// from_item, the entire workitems.Create transaction rolls back.
	env := callTool(t, fx.RawKey, "create", map[string]any{
		"project_id": fx.ProjectID,
		"type":       "task",
		"title":      "should not persist",
		"dependencies": []map[string]any{
			{"blocker_item_id": "01ZZZZZZZZZZZZZZZZZZZZZZZZ", "kind": "blocks"},
		},
	})
	if env.Error == nil {
		t.Fatalf("expected error on bogus blocker_item_id; got success result=%s", string(env.Result))
	}
	var data envelopeData
	if err := json.Unmarshal(env.Error.Data, &data); err != nil {
		t.Fatalf("unmarshal error.data: %v", err)
	}
	if data.Kind != "NOT_FOUND" {
		t.Fatalf("error.data.kind = %q, want NOT_FOUND", data.Kind)
	}

	// Atomicity assertion: no workitems.items row with the test title
	// exists. If the pre-D-2 phantom-item bug were still present, the
	// item insert would have committed before the edge loop and we'd
	// find a row here.
	ctx := context.Background()
	var count int
	if err := db.QueryRow(ctx,
		`SELECT COUNT(*) FROM workitems.items WHERE title = $1 AND org_id = $2`,
		"should not persist", fx.OrgID,
	).Scan(&count); err != nil {
		t.Fatalf("count items: %v", err)
	}
	if count != 0 {
		t.Fatalf("phantom item left behind: count = %d, want 0", count)
	}
}

// TestD2_AuditRowsCarryToolName asserts the §8.1 contract that every
// authenticated dispatch writes one mcp.tool_calls row with the
// canonical tool_name (NEVER "transport" — that is the pre-tool
// baseline only).
func TestD2_AuditRowsCarryToolName(t *testing.T) {
	resetToolCalls(t)
	fx := seedD2Fixture(t)
	seedReadyItem(t, fx.OrgID, fx.ProjectID, "P0", 0)

	// One of each tool: prime, ready, claim, create.
	_ = callTool(t, fx.RawKey, "prime", map[string]any{"project_id": fx.ProjectID})
	_ = callTool(t, fx.RawKey, "ready", map[string]any{"project_id": fx.ProjectID})
	createEnv := callTool(t, fx.RawKey, "create", map[string]any{
		"project_id": fx.ProjectID,
		"type":       "task",
		"title":      "audit test item",
	})
	res := assertStructuredEchoesText(t, createEnv)
	var createOut struct {
		Item struct {
			ID string `json:"id"`
		} `json:"item"`
	}
	if err := json.Unmarshal(res.StructuredContent, &createOut); err != nil {
		t.Fatalf("unmarshal create: %v", err)
	}

	// Build a fresh Ready item for claim (the created one above lands
	// as Backlog, not Ready).
	claimable := seedReadyItem(t, fx.OrgID, fx.ProjectID, "P1", 2*time.Second)
	_ = callTool(t, fx.RawKey, "claim", map[string]any{"item_id": claimable})

	rows := selectToolCalls(t)
	have := map[string]int{}
	for _, r := range rows {
		have[r.ToolName]++
	}
	for _, want := range []string{"prime", "ready", "create", "claim"} {
		if have[want] < 1 {
			t.Fatalf("audit row for tool_name=%q: count=%d, want >=1; rows=%+v", want, have[want], rows)
		}
	}
}
