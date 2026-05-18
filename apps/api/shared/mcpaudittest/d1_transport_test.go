// d1_transport_test.go covers the D-1 (unblock-tv8.16) acceptance
// matrix for the MCP Streamable HTTP transport at /mcp:
//
//  1. POST /mcp with a valid Bearer API key + JSON-RPC `initialize`
//     body returns HTTP 200 and a non-empty Mcp-Session-Id header.
//  2. GET /mcp with a valid Bearer API key + Mcp-Session-Id returns
//     HTTP 200 + Content-Type starting with `text/event-stream`.
//  3. PUT/PATCH/DELETE /mcp returns HTTP 405 + Allow: POST, GET —
//     regardless of Authorization header presence (the method-not-
//     allowed branch runs BEFORE auth, per SPEC §4.3.1 + bead AC).
//  4. POST /mcp without Authorization returns HTTP 200 with a
//     JSON-RPC error envelope whose data.kind == "UNAUTHENTICATED"
//     (SPEC §7).
//
// Transport shape: the suite drives the MCP handler logic via
// httptest.NewServer wrapping mcp.ServeMCPForTest (see
// apps/api/mcp/export_test_writer.go). The A-5 DEVIATION recorded
// in mcpaudittest_test.go:62-75 is still present in encore.dev
// v1.52.1: `encore test` does not route raw //encore:api endpoints
// on the in-process listener, and E1387/E1389 forbid in-process
// references to MCPHandler from any code in the Encore application
// graph. The mcp package therefore splits MCPHandler (the thin
// //encore:api wrapper) from serveMCP (the implementation) and
// exports serveMCP under the test-only name ServeMCPForTest for
// this suite to wrap in httptest. The wrapped handler exercises
// the identical code path that production traffic hits — the
// split is purely a testability seam, not a separate code path.
//
// Seed strategy: each test mints a fresh org + a fresh
// mcp.api_keys row via auth.IssueAPIKey so the Bearer hot path
// exercises the real §4.3.2 lookup chain (key_prefix UNIQUE index
// + HMAC compare). Tests cannot share an org because mcp.tool_calls
// has a FK to org.organizations — resetting tool_calls between
// tests is the existing pattern (see resetToolCalls).

package mcpaudittest

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"encore.app/auth"
	"encore.app/mcp"
)

// mcpInitializeBody is a minimal JSON-RPC 2.0 initialize request per
// MCP spec 2025-06-18. The SDK rejects the request without `params`
// matching its InitializeParams shape, so we provide the fields the
// SDK requires: protocolVersion, capabilities, clientInfo.
//
// Kept as a constant rather than a marshalled struct so the test
// drives the on-wire shape exactly — a Go-side struct that the SDK
// later changes would be a silent test drift surface.
const mcpInitializeBody = `{
	"jsonrpc": "2.0",
	"id": 1,
	"method": "initialize",
	"params": {
		"protocolVersion": "2025-06-18",
		"capabilities": {},
		"clientInfo": {
			"name": "d1-transport-test",
			"version": "0.0.1"
		}
	}
}`

// mcpInitializeTimeout bounds the wall-clock budget for a single
// MCP `initialize` round-trip in the D-1 suite. The 5s default that
// peer auth-only tests use is too tight here: the initialize path
// pays the full cold-start cost the first time it runs in the suite
// — Docker Postgres warmup, migrations, seedOrg INSERT, the
// auth.IssueAPIKey path (HMAC + INSERT into mcp.api_keys), the SDK
// Connect() handshake, and the in-process Bearer hot path lookup.
// On a CPU-throttled or contended host that easily exceeds 5s, which
// surfaces as `read body: context deadline exceeded` at io.ReadAll
// (the client cancels the request ctx, the deferred recordToolCall
// then runs against the canceled ctx and emits a fire-and-forget
// `recordToolCall: insert failed err="canceled: context canceled"`
// log line — symptom, not cause).
//
// 30s is the spec-aligned headroom: SPEC §11.2 NFR-1 measures
// latency on a warm local emulator, so the integration test that
// includes the seed work is explicitly outside that budget. The
// peer test `callTool` at d2_tools_test.go:199 uses 10s for a
// session-resuming tools/call; the initialize variant runs first
// and pays the higher cold-start cost, so it gets the wider margin.
// If a future regression actually causes the server to hang, the
// test still bounds the suite — it just gives the cold path room
// to land first.
const mcpInitializeTimeout = 30 * time.Second

// mcpTestServer is a process-wide httptest.Server wrapping
// mcp.ServeMCPForTest. Created lazily on first use so test packages
// that exercise only the audit writer (TestRecordToolCallPersistsRow
// and friends) do not pay the listener-open cost.
//
// The server lives for the lifetime of the test binary — net/http
// reuses a single listener across the suite, and the suite cannot
// run tests in parallel that share state through mcp.tool_calls
// anyway (resetToolCalls + seedOrg use shared schemas). The
// goroutine started by httptest.NewServer is reaped on process
// exit; the SDK's session map is reset between tests by minting a
// fresh Mcp-Session-Id (every initialize handshake creates a new
// session).
var (
	mcpTestServerOnce sync.Once
	mcpTestServer     *httptest.Server
)

// mcpEndpoint returns the absolute URL of the MCP test endpoint
// served by httptest. See the A-5 DEVIATION + the file-level
// doc-comment for why we cannot use encore.Meta().APIBaseURL +
// "/mcp" — encore's in-process test listener does not register
// raw //encore:api routes.
func mcpEndpoint() string {
	mcpTestServerOnce.Do(func() {
		mcpTestServer = httptest.NewServer(http.HandlerFunc(mcp.ServeMCPForTest))
	})
	return mcpTestServer.URL
}

// seedAPIKey mints a fresh mcp.api_keys row scoped to orgID and
// returns the raw key string + the key id. The Bearer auth hot path
// (auth.go validateAPIKey) reads key_prefix + HMAC-SHA256(secret,
// rawKey); IssueAPIKey performs the full key-format dance so we
// exercise the real production path.
//
// Each test calls seedAPIKey with its own org id so tests run
// independently (no cross-test fixture coupling).
func seedAPIKey(t *testing.T, orgID string) (rawKey, keyID string) {
	t.Helper()
	resp, err := auth.IssueAPIKey(context.Background(), &auth.IssueAPIKeyRequest{
		OrgID:     orgID,
		Label:     "d1-transport-test",
		AgentKind: "claude-code",
		Scopes:    []string{},
	})
	if err != nil {
		t.Fatalf("auth.IssueAPIKey: %v", err)
	}
	return resp.RawKey, resp.KeyID
}

// httpDo runs a single HTTP request with a short per-test timeout
// so a hung server does not deadlock the suite. The client follows
// no redirects (MCP transport never emits 3xx) and uses a fresh
// connection per call to avoid keep-alive state bleeding across
// tests.
func httpDo(t *testing.T, req *http.Request, timeout time.Duration) *http.Response {
	t.Helper()
	client := &http.Client{
		Timeout: timeout,
		CheckRedirect: func(*http.Request, []*http.Request) error {
			return http.ErrUseLastResponse
		},
	}
	resp, err := client.Do(req)
	if err != nil {
		t.Fatalf("http.Do: %v", err)
	}
	return resp
}

// TestD1_POSTInitializeReturnsSessionID covers AC #1.
//
// POST /mcp with a valid Bearer API key + JSON-RPC initialize body
// returns HTTP 200 and a non-empty Mcp-Session-Id header. The header
// is minted by the SDK's session manager on the initialize
// handshake; subsequent requests echo it.
func TestD1_POSTInitializeReturnsSessionID(t *testing.T) {
	resetToolCalls(t)
	orgID := seedOrg(t)
	rawKey, _ := seedAPIKey(t, orgID)

	req, err := http.NewRequest(http.MethodPost, mcpEndpoint(), strings.NewReader(mcpInitializeBody))
	if err != nil {
		t.Fatalf("http.NewRequest: %v", err)
	}
	req.Header.Set("Content-Type", "application/json")
	// SPEC §5.1: the client must indicate willingness to receive
	// either a single JSON body or an SSE stream — the SDK rejects
	// requests that omit one of the two.
	req.Header.Set("Accept", "application/json, text/event-stream")
	req.Header.Set("Authorization", "Bearer "+rawKey)

	resp := httpDo(t, req, mcpInitializeTimeout)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("status = %d, want 200; body=%s", resp.StatusCode, string(body))
	}
	sessionID := resp.Header.Get("Mcp-Session-Id")
	if sessionID == "" {
		t.Fatalf("Mcp-Session-Id header missing on initialize response")
	}

	// Sanity check the body is a JSON-RPC initialize response.
	// We do NOT validate the SDK's response shape exhaustively
	// (the SDK has its own conformance tests); just confirm the
	// envelope parses as JSON and carries a `result` field with
	// `protocolVersion`.
	bodyBytes, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatalf("read body: %v", err)
	}
	var env struct {
		JSONRPC string `json:"jsonrpc"`
		ID      any    `json:"id"`
		Result  struct {
			ProtocolVersion string `json:"protocolVersion"`
		} `json:"result"`
		Error any `json:"error"`
	}
	// The SDK may emit the response as either application/json or
	// text/event-stream depending on Accept negotiation. We accept
	// both: for SSE we extract the data: line; for JSON we parse
	// directly.
	parsed := bodyBytes
	contentType := resp.Header.Get("Content-Type")
	if strings.HasPrefix(contentType, "text/event-stream") {
		parsed = extractFirstSSEData(t, bodyBytes)
	}
	if err := json.Unmarshal(parsed, &env); err != nil {
		t.Fatalf("unmarshal initialize response: %v; body=%s", err, string(bodyBytes))
	}
	if env.Error != nil {
		t.Fatalf("initialize response carried error: %v", env.Error)
	}
	if env.JSONRPC != "2.0" {
		t.Fatalf("jsonrpc = %q, want \"2.0\"", env.JSONRPC)
	}
	if env.Result.ProtocolVersion == "" {
		t.Fatalf("result.protocolVersion is empty; body=%s", string(parsed))
	}
}

// TestD1_GETOpensSSEStream covers AC #2.
//
// GET /mcp with a valid Bearer + Mcp-Session-Id returns HTTP 200
// and Content-Type starting with text/event-stream. We do not read
// keepalive frames here — the SDK's KeepAlive option emits JSON-RPC
// pings at the configured cadence which serve the same purpose;
// asserting Content-Type proves the stream opened (the SDK only
// emits that header on a successful SSE handshake).
//
// A previous draft of this test attempted to assert a keepalive
// frame appeared within ~16s. That coupled the test to the SDK's
// internal frame format (events vs. SSE comments) AND added a
// real-time wait to the suite. The Content-Type assertion is the
// canonical "stream opened" signal — the §5.1 keepalive cadence
// (15s) is best validated in a production smoke test, not a unit
// suite.
func TestD1_GETOpensSSEStream(t *testing.T) {
	resetToolCalls(t)
	orgID := seedOrg(t)
	rawKey, _ := seedAPIKey(t, orgID)

	// Initialize first to obtain a session id — the SDK rejects GET
	// without an existing session in stateful mode.
	postReq, err := http.NewRequest(http.MethodPost, mcpEndpoint(), strings.NewReader(mcpInitializeBody))
	if err != nil {
		t.Fatalf("http.NewRequest POST: %v", err)
	}
	postReq.Header.Set("Content-Type", "application/json")
	postReq.Header.Set("Accept", "application/json, text/event-stream")
	postReq.Header.Set("Authorization", "Bearer "+rawKey)

	// Use the same wide budget as the dedicated initialize test —
	// this POST pays the same cold-start cost (seedOrg + IssueAPIKey
	// + SDK Connect) and was a coupled flake source.
	postResp := httpDo(t, postReq, mcpInitializeTimeout)
	sessionID := postResp.Header.Get("Mcp-Session-Id")
	postResp.Body.Close()
	if sessionID == "" {
		t.Fatalf("initialize must return Mcp-Session-Id; got empty")
	}

	// Now open the GET stream. The test client times out before the
	// SDK's first keepalive fires (15s) — we are only checking the
	// handshake.
	getReq, err := http.NewRequest(http.MethodGet, mcpEndpoint(), nil)
	if err != nil {
		t.Fatalf("http.NewRequest GET: %v", err)
	}
	getReq.Header.Set("Accept", "text/event-stream")
	getReq.Header.Set("Authorization", "Bearer "+rawKey)
	getReq.Header.Set("Mcp-Session-Id", sessionID)

	// Long timeout — the SDK keeps the SSE stream open until the
	// client closes. We close it after reading the response
	// headers; the deadline only fires on a stuck handshake.
	resp := httpDo(t, getReq, 10*time.Second)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 4096))
		t.Fatalf("GET status = %d, want 200; body=%s", resp.StatusCode, string(body))
	}
	contentType := resp.Header.Get("Content-Type")
	if !strings.HasPrefix(contentType, "text/event-stream") {
		t.Fatalf("Content-Type = %q, want prefix \"text/event-stream\"", contentType)
	}
}

// TestD1_NonPOSTGETReturns405 covers AC #3.
//
// PUT, PATCH, DELETE /mcp return HTTP 405 with the Allow header
// `POST, GET`. The 405 short-circuit fires BEFORE auth (SPEC §4.3.1
// + bead AC) so we omit the Authorization header to prove the
// independence; sending one would not change the outcome.
//
// SDK note: the SDK's own ServeHTTP returns 405 with
// `Allow: GET, POST, DELETE` in stateful mode for unknown methods.
// MCPHandler's method filter runs BEFORE the SDK so the spec-locked
// `Allow: POST, GET` reaches the wire — DELETE is NOT a P01-exposed
// method (the SDK's session-close path is reachable only via
// shutdown, not a direct DELETE — D-1 does not expose it).
func TestD1_NonPOSTGETReturns405(t *testing.T) {
	for _, method := range []string{http.MethodPut, http.MethodPatch, http.MethodDelete} {
		t.Run(method, func(t *testing.T) {
			req, err := http.NewRequest(method, mcpEndpoint(), nil)
			if err != nil {
				t.Fatalf("http.NewRequest %s: %v", method, err)
			}

			resp := httpDo(t, req, 5*time.Second)
			defer resp.Body.Close()

			if resp.StatusCode != http.StatusMethodNotAllowed {
				body, _ := io.ReadAll(resp.Body)
				t.Fatalf("%s status = %d, want 405; body=%s", method, resp.StatusCode, string(body))
			}
			allow := resp.Header.Get("Allow")
			if allow != "POST, GET" {
				t.Fatalf("%s Allow = %q, want \"POST, GET\"", method, allow)
			}
		})
	}
}

// TestD1_POSTNoAuthReturnsUnauthenticated covers AC #4.
//
// POST /mcp without Authorization returns HTTP 200 (JSON-RPC
// errors travel inside the body, never as HTTP status codes) with
// a §7 error envelope whose data.kind == "UNAUTHENTICATED".
//
// We also assert no mcp.tool_calls row is written for the
// auth-failure path (DECISION 3 on the bead — mcp.tool_calls.org_id
// is FK + NOT NULL, and recordToolCall short-circuits when OrgID
// is empty).
func TestD1_POSTNoAuthReturnsUnauthenticated(t *testing.T) {
	resetToolCalls(t)

	req, err := http.NewRequest(http.MethodPost, mcpEndpoint(), strings.NewReader(mcpInitializeBody))
	if err != nil {
		t.Fatalf("http.NewRequest: %v", err)
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json, text/event-stream")
	// Deliberately no Authorization header.

	resp := httpDo(t, req, 5*time.Second)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("status = %d, want 200; body=%s", resp.StatusCode, string(body))
	}
	contentType := resp.Header.Get("Content-Type")
	if !strings.HasPrefix(contentType, "application/json") {
		t.Fatalf("Content-Type = %q, want prefix \"application/json\"", contentType)
	}

	bodyBytes, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatalf("read body: %v", err)
	}
	var env struct {
		JSONRPC string `json:"jsonrpc"`
		ID      any    `json:"id"`
		Error   struct {
			Code    int    `json:"code"`
			Message string `json:"message"`
			Data    struct {
				Kind    string `json:"kind"`
				Tool    string `json:"tool"`
				TraceID string `json:"trace_id"`
			} `json:"data"`
		} `json:"error"`
	}
	if err := json.Unmarshal(bodyBytes, &env); err != nil {
		t.Fatalf("unmarshal envelope: %v; body=%s", err, string(bodyBytes))
	}
	if env.JSONRPC != "2.0" {
		t.Fatalf("jsonrpc = %q, want \"2.0\"", env.JSONRPC)
	}
	if env.Error.Code != -32000 {
		t.Fatalf("error.code = %d, want -32000", env.Error.Code)
	}
	if env.Error.Data.Kind != "UNAUTHENTICATED" {
		t.Fatalf("error.data.kind = %q, want \"UNAUTHENTICATED\"", env.Error.Data.Kind)
	}
	// trace_id should be a 26-char Crockford-base32 ULID — the
	// envelope writer pulls it from tracectx, which MCPHandler
	// populated at request entry before auth ran.
	if len(env.Error.Data.TraceID) != 26 {
		t.Fatalf("error.data.trace_id length = %d, want 26 (ULID); got %q", len(env.Error.Data.TraceID), env.Error.Data.TraceID)
	}

	// Audit-row contract: no mcp.tool_calls row for an auth-failure
	// dispatch (DECISION 3). The pre-auth recordToolCall path
	// short-circuits on the empty OrgID.
	rows := selectToolCalls(t)
	if len(rows) != 0 {
		t.Fatalf("tool_calls rows = %d, want 0 (no audit row on auth-failure)", len(rows))
	}
}

// extractFirstSSEData lifts the first `data: ` payload from an SSE
// frame stream. Used by TestD1_POSTInitializeReturnsSessionID when
// the SDK chose text/event-stream over application/json for the
// initialize response body. SSE format per RFC + WHATWG: lines
// starting with `data: ` carry the payload; consecutive data lines
// are joined with newlines; the frame terminates on an empty line.
func extractFirstSSEData(t *testing.T, body []byte) []byte {
	t.Helper()
	var buf bytes.Buffer
	for _, line := range bytes.Split(body, []byte("\n")) {
		line = bytes.TrimRight(line, "\r")
		if len(line) == 0 {
			if buf.Len() > 0 {
				return buf.Bytes()
			}
			continue
		}
		if bytes.HasPrefix(line, []byte("data: ")) {
			if buf.Len() > 0 {
				buf.WriteByte('\n')
			}
			buf.Write(line[len("data: "):])
		}
	}
	if buf.Len() == 0 {
		t.Fatalf("no SSE data: payload in body; got=%s", string(body))
	}
	return buf.Bytes()
}
