// transport_test.go owns the MCP-tool dispatch transport that the
// §11.1.2 functional assertions and §11.3 architectural invariants
// drive. The pattern mirrors apps/api/shared/mcpaudittest/
// d1_transport_test.go + d2_tools_test.go — lazy httptest singleton
// wrapping mcp.ServeMCPForTest, JSON-RPC initialize + tools/call,
// SSE-data extraction when the SDK negotiates text/event-stream.
//
// The KEY divergence from mcpaudittest: this suite NEVER calls
// auth.IssueAPIKey. The Bearer token is the in-memory raw key minted
// by the seed (Fixture.RawKey), and the seed's INSERT into
// mcp.api_keys uses the production HMAC under
// secrets.APIKeyHMACSecret (SPEC §11.1.1).

package exitcriteriontest_test

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"encore.app/mcp"
)

// mcpInitializeBody is a minimal JSON-RPC 2.0 initialize request
// per MCP spec 2025-06-18. The SDK rejects requests without `params`
// matching its InitializeParams shape, so we provide the fields the
// SDK requires: protocolVersion, capabilities, clientInfo. Same
// constant shape as apps/api/shared/mcpaudittest/d1_transport_test.go:62-74.
const mcpInitializeBody = `{
	"jsonrpc": "2.0",
	"id": 1,
	"method": "initialize",
	"params": {
		"protocolVersion": "2025-06-18",
		"capabilities": {},
		"clientInfo": {
			"name": "exit-criterion-test",
			"version": "0.0.1"
		}
	}
}`

// mcpInitializeTimeout bounds the cold-start budget for a single
// MCP initialize round-trip. 30s mirrors mcpaudittest's value (same
// SDK Connect + first-call Bearer hot path warm-up cost; see
// d1_transport_test.go:76-99 for the rationale).
const mcpInitializeTimeout = 30 * time.Second

// mcpToolCallTimeout bounds a single tools/call round-trip on a
// pre-initialized session. 10s matches mcpaudittest's value.
const mcpToolCallTimeout = 10 * time.Second

// mcpTestServer is a process-wide httptest.Server wrapping
// mcp.ServeMCPForTest. Lazy singleton because not every test body
// exercises the MCP transport (the cascade-row subscriber-driver
// tests do not need it). Same shape as mcpaudittest's
// mcpTestServer.
var (
	mcpTestServerOnce sync.Once
	mcpTestServer     *httptest.Server
)

// mcpEndpoint returns the absolute URL of the MCP test endpoint.
// See doc.go's "Bearer auth + tool dispatch transport" section for
// why we cannot use encore.Meta().APIBaseURL + "/mcp".
func mcpEndpoint() string {
	mcpTestServerOnce.Do(func() {
		mcpTestServer = httptest.NewServer(http.HandlerFunc(mcp.ServeMCPForTest))
	})
	return mcpTestServer.URL
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

// initializeSession opens a fresh MCP session against the test
// server with the given Bearer token. Returns the Mcp-Session-Id
// header value. Tests that drive multiple tools/call invocations on
// the same identity call initializeSession once and reuse the
// session id across subsequent callTool invocations.
func initializeSession(t *testing.T, rawKey string) string {
	t.Helper()

	req, err := http.NewRequest(http.MethodPost, mcpEndpoint(), strings.NewReader(mcpInitializeBody))
	if err != nil {
		t.Fatalf("http.NewRequest initialize: %v", err)
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json, text/event-stream")
	req.Header.Set("Authorization", "Bearer "+rawKey)

	resp := httpDo(t, req, mcpInitializeTimeout)
	defer func() { _ = resp.Body.Close() }()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("initialize status = %d, want 200; body=%s", resp.StatusCode, string(body))
	}
	sessionID := resp.Header.Get("Mcp-Session-Id")
	if sessionID == "" {
		t.Fatalf("initialize did not return Mcp-Session-Id")
	}
	// Drain the body so the connection can be reused / closed cleanly.
	_, _ = io.Copy(io.Discard, resp.Body)
	return sessionID
}

// jsonRPCEnvelope is the test-side shape of a JSON-RPC response.
// result/error decoded as RawMessage so each test picks the right
// sub-shape and validates it against the spec. Same shape as
// mcpaudittest's jsonRPCEnvelope.
type jsonRPCEnvelope struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      any             `json:"id"`
	Result  json.RawMessage `json:"result,omitempty"`
	Error   *envelopeError  `json:"error,omitempty"`
}

// envelopeError mirrors the JSON-RPC error object shape. data is
// the §7 envelope payload (kind, tool, trace_id, details).
type envelopeError struct {
	Code    int             `json:"code"`
	Message string          `json:"message"`
	Data    json.RawMessage `json:"data"`
}

// envelopeData mirrors the §7 envelope's data field shape so test
// bodies can assert on data.kind without re-marshalling.
type envelopeData struct {
	Kind    string         `json:"kind"`
	Tool    string         `json:"tool"`
	TraceID string         `json:"trace_id"`
	Details map[string]any `json:"details"`
}

// toolCallResult is the structured shape the SDK emits for a
// successful tool dispatch. content[0].text MUST be a JSON string
// that round-trips to structuredContent — that is the §6.1 framing
// invariant.
type toolCallResult struct {
	Content []struct {
		Type string `json:"type"`
		Text string `json:"text"`
	} `json:"content"`
	StructuredContent json.RawMessage `json:"structuredContent"`
	IsError           bool            `json:"isError"`
}

// callTool drives a JSON-RPC tools/call against the MCP test server
// on an existing session. Returns the parsed envelope; caller walks
// result vs error. Used by all §11.1.2 tool-surface assertions.
//
// Distinct from mcpaudittest's callTool because we accept an
// existing sessionID rather than running a fresh initialize per
// call — the §11.1.2 assertions walk through a sequence (prime →
// ready → claim → close → prime) that must share one session so the
// SDK's stateful session map sees consecutive requests.
func callTool(t *testing.T, rawKey, sessionID, toolName string, arguments any) jsonRPCEnvelope {
	t.Helper()

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

	resp := httpDo(t, req, mcpToolCallTimeout)
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("tools/call %q status = %d, want 200; body=%s", toolName, resp.StatusCode, string(body))
	}

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatalf("read body: %v", err)
	}
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

// expectSuccess unwraps a successful tool-call envelope into the
// structured content payload. Fails the test if the envelope
// carries an error or isError=true. Returns the canonical
// structuredContent bytes (caller picks the right typed shape).
func expectSuccess(t *testing.T, env jsonRPCEnvelope) []byte {
	t.Helper()
	if env.Error != nil {
		t.Fatalf("expected success, got error: code=%d message=%s data=%s",
			env.Error.Code, env.Error.Message, string(env.Error.Data))
	}
	var res toolCallResult
	if err := json.Unmarshal(env.Result, &res); err != nil {
		t.Fatalf("unmarshal result: %v; body=%s", err, string(env.Result))
	}
	if res.IsError {
		t.Fatalf("isError = true on success path; structured=%s", string(res.StructuredContent))
	}
	return res.StructuredContent
}

// expectError unwraps a §7-envelope error from a tools/call response
// (the envelope error carries a `data` field with kind/tool/details).
// Returns the parsed envelopeData so test bodies can assert on
// data.kind == "CYCLE_DETECTED" / "ALREADY_CLAIMED" / etc.
func expectError(t *testing.T, env jsonRPCEnvelope) envelopeData {
	t.Helper()
	if env.Error == nil {
		t.Fatalf("expected error envelope, got success: result=%s", string(env.Result))
	}
	var data envelopeData
	if err := json.Unmarshal(env.Error.Data, &data); err != nil {
		t.Fatalf("unmarshal error data: %v; raw=%s", err, string(env.Error.Data))
	}
	return data
}

// extractFirstSSEData lifts the first `data: ` payload from an SSE
// frame stream. Used when the SDK chose text/event-stream over
// application/json for the response body. Same shape as
// mcpaudittest's extractFirstSSEData.
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
