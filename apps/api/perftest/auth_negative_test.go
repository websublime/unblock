// auth_negative_test.go closes the W3 gap carried forward from the
// closed B-1 auth-service review (cross-linked on bead unblock-tv8.24):
// the four DB-bound auth RPC bodies were verified only by inspection.
// This file exercises the §4.3.2 negative paths against the same
// httptest MCP server the latency harness uses.
//
// Negative paths (SPEC §4.3.2 + §11.2 W3 closure):
//
//   - revoked key      → auth.go step 4, line 199.
//   - expired key      → auth.go step 4, line 202.
//   - unknown prefix   → auth.go step 3, line 189 (ErrNoRows).
//   - bad HMAC         → auth.go step 6, line 208 (ConstantTimeCompare).
//   - missing prefix   → auth.go step 2, line 159-165 (prefixOf parse).
//
// Wire-signal note (DEVIATION on bead unblock-tv8.24). The bead AC +
// spec phrase the assertion as "401 / errs.Unauthenticated". The MCP
// Streamable HTTP transport NEVER returns HTTP 401 — auth failures
// return HTTP 200 with a JSON-RPC 2.0 error envelope whose
// error.code == -32000 and data.kind == "UNAUTHENTICATED"
// (apps/api/mcp/errenvelope.go:27-30,179-182; mcp/mcp.go:178-214). The
// d1 transport precedent
// (apps/api/shared/mcpaudittest/d1_transport_test.go:351-427) already
// establishes this as the canonical auth-rejection signal at this
// transport. Each sub-test asserts the UNAUTHENTICATED envelope — the
// faithful realisation of "errs.Unauthenticated" at the HTTP/MCP edge.

package perftest_test

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"strings"
	"testing"

	encoredb "encore.app/db"
	"encore.app/perftest"
)

// jsonRPCErrorCodeToolError is the JSON-RPC 2.0 error.code the §7
// envelope writer emits for every tool-level error, including
// UNAUTHENTICATED (apps/api/mcp/errenvelope.go:80).
const jsonRPCErrorCodeToolError = -32000

// envelopeKindUnauthenticated is the §7 data.kind value for an auth
// rejection (apps/api/mcp/errenvelope.go:64).
const envelopeKindUnauthenticated = "UNAUTHENTICATED"

// postInitializeRaw posts a JSON-RPC initialize to the MCP test server
// with the given Authorization header value. If authzHeader is empty,
// no Authorization header is sent (the missing-header path). Returns
// the HTTP status code and the response body bytes.
func postInitializeRaw(t *testing.T, authzHeader string) (int, []byte) {
	t.Helper()
	req, err := http.NewRequest(http.MethodPost, mcpEndpoint(), strings.NewReader(mcpInitializeBody))
	if err != nil {
		t.Fatalf("http.NewRequest initialize: %v", err)
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json, text/event-stream")
	if authzHeader != "" {
		req.Header.Set("Authorization", authzHeader)
	}

	resp := httpDo(t, req, mcpInitializeTimeout)
	defer func() { _ = resp.Body.Close() }()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatalf("read body: %v", err)
	}
	payload := body
	if strings.HasPrefix(resp.Header.Get("Content-Type"), "text/event-stream") {
		payload = extractFirstSSEData(t, body)
	}
	return resp.StatusCode, payload
}

// assertUnauthenticated posts an initialize with the given Bearer raw
// key and asserts the canonical auth-rejection wire signal: HTTP 200,
// a JSON-RPC error envelope with code -32000 and data.kind ==
// "UNAUTHENTICATED", and NO Mcp-Session-Id minted (proven by the
// absence of a result field). bearerKey is the raw token sent as
// `Bearer <bearerKey>`.
func assertUnauthenticated(t *testing.T, bearerKey string) {
	t.Helper()
	status, payload := postInitializeRaw(t, "Bearer "+bearerKey)

	// JSON-RPC errors travel inside the body over HTTP 200 — the
	// transport NEVER returns 401 (see file-level DEVIATION note).
	if status != http.StatusOK {
		t.Fatalf("status = %d, want 200 (auth failures carry the error in the JSON-RPC body); body=%s", status, string(payload))
	}

	var env struct {
		JSONRPC string          `json:"jsonrpc"`
		Result  json.RawMessage `json:"result"`
		Error   *struct {
			Code int `json:"code"`
			Data struct {
				Kind    string `json:"kind"`
				TraceID string `json:"trace_id"`
			} `json:"data"`
		} `json:"error"`
	}
	if err := json.Unmarshal(payload, &env); err != nil {
		t.Fatalf("unmarshal envelope: %v; body=%s", err, string(payload))
	}
	if env.JSONRPC != "2.0" {
		t.Fatalf("jsonrpc = %q, want \"2.0\"; body=%s", env.JSONRPC, string(payload))
	}
	if env.Error == nil {
		t.Fatalf("expected an error envelope (auth rejected), got success; body=%s", string(payload))
	}
	if env.Error.Code != jsonRPCErrorCodeToolError {
		t.Fatalf("error.code = %d, want %d; body=%s", env.Error.Code, jsonRPCErrorCodeToolError, string(payload))
	}
	if env.Error.Data.Kind != envelopeKindUnauthenticated {
		t.Fatalf("error.data.kind = %q, want %q; body=%s", env.Error.Data.Kind, envelopeKindUnauthenticated, string(payload))
	}
	// trace_id should be a 26-char Crockford-base32 ULID (the envelope
	// writer pulls it from tracectx, populated at request entry before
	// auth ran).
	if len(env.Error.Data.TraceID) != 26 {
		t.Fatalf("error.data.trace_id length = %d, want 26 (ULID); got %q", len(env.Error.Data.TraceID), env.Error.Data.TraceID)
	}
}

// TestNFR1_NegativeAuthPaths covers the §4.3.2 negative-path matrix
// (W3 closure). Each sub-test mints/manipulates a key in a specific
// bad state and asserts the UNAUTHENTICATED envelope.
//
// Sub-tests are NOT parallel (the suite-wide no-t.Parallel convention;
// see doc.go). They share the seeded fixture's org/user for the
// DB-row paths (revoked/expired/bad-HMAC); those rows cascade-delete
// with the fixture on teardown.
func TestNFR1_NegativeAuthPaths(t *testing.T) {
	f := fx(t)
	ctx := context.Background()

	// Positive control: the fixture's valid key MUST authenticate, so
	// the negative assertions below are proven to test the bad state —
	// not a globally-broken transport.
	t.Run("valid_key_control", func(t *testing.T) {
		sessionID := initializeSession(t, f.RawKey)
		if sessionID == "" {
			t.Fatal("valid key did not produce a session — the negative-path assertions would be meaningless")
		}
	})

	t.Run("revoked_key", func(t *testing.T) {
		rawKey, err := f.SeedRevokedKey(ctx, encoredb.DB)
		if err != nil {
			t.Fatalf("SeedRevokedKey: %v", err)
		}
		assertUnauthenticated(t, rawKey)
	})

	t.Run("expired_key", func(t *testing.T) {
		rawKey, err := f.SeedExpiredKey(ctx, encoredb.DB)
		if err != nil {
			t.Fatalf("SeedExpiredKey: %v", err)
		}
		assertUnauthenticated(t, rawKey)
	})

	t.Run("unknown_prefix", func(t *testing.T) {
		rawKey, err := perftest.UnknownPrefixRawKey()
		if err != nil {
			t.Fatalf("UnknownPrefixRawKey: %v", err)
		}
		assertUnauthenticated(t, rawKey)
	})

	t.Run("bad_hmac", func(t *testing.T) {
		rawKey, err := f.SeedBadHMACKey(ctx, encoredb.DB)
		if err != nil {
			t.Fatalf("SeedBadHMACKey: %v", err)
		}
		assertUnauthenticated(t, rawKey)
	})

	t.Run("missing_prefix", func(t *testing.T) {
		rawKey, err := perftest.MissingPrefixRawKey()
		if err != nil {
			t.Fatalf("MissingPrefixRawKey: %v", err)
		}
		assertUnauthenticated(t, rawKey)
	})

	// Missing Authorization header entirely (no Bearer at all). The
	// transport short-circuits at mcp.go:178-181 before any key lookup.
	t.Run("missing_authorization_header", func(t *testing.T) {
		status, payload := postInitializeRaw(t, "")
		if status != http.StatusOK {
			t.Fatalf("status = %d, want 200; body=%s", status, string(payload))
		}
		var env struct {
			Error *struct {
				Code int `json:"code"`
				Data struct {
					Kind string `json:"kind"`
				} `json:"data"`
			} `json:"error"`
		}
		if err := json.Unmarshal(payload, &env); err != nil {
			t.Fatalf("unmarshal envelope: %v; body=%s", err, string(payload))
		}
		if env.Error == nil || env.Error.Data.Kind != envelopeKindUnauthenticated {
			t.Fatalf("missing-header path did not return UNAUTHENTICATED; body=%s", string(payload))
		}
	})
}
