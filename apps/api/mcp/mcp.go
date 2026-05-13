// Package mcp owns the public Streamable HTTP MCP endpoint at /mcp and
// the 14 P01 tool handlers. See SPEC §4.3 (transport, auth, hot path)
// and §6.2 (tool catalogue).
//
// In P01 task A-1 this package only declared the //encore:api raw
// endpoint skeleton so Encore recognised mcp as a service. Bead A-5
// (unblock-tv8.5) layered the cross-cutting tracing scaffold on top:
// ULID trace_id mint, tracectx binding, deferred recordToolCall.
//
// Bead D-1 (unblock-tv8.16) lands the transport body: the Go MCP SDK
// (github.com/modelcontextprotocol/go-sdk v1.6.0) StreamableHTTPHandler
// is constructed once at package init (see transport.go) and MCPHandler
// in this file is the Encore wrapper that does:
//
//  1. HTTP-method filtering: POST + GET delegate to the SDK; all
//     other methods short-circuit with 405 + Allow: POST, GET
//     (BEFORE auth runs — AC #3).
//  2. Bearer parse + auth.Validate() in-package call (DECISION 1 on
//     the bead — shape (b) from the investigation): keep
//     `//encore:api public raw path=/mcp` (Encore v1.52.1 rejects
//     the literal `method=*` from SPEC §4.3.1's sample with E1371
//     "Invalid endpoint method"; the raw-endpoint default per
//     ENCORE.md is to match every HTTP method when `method=` is
//     omitted — functionally identical to the `method=*` intent),
//     do the auth manually inside the handler so the 405 path can
//     bypass auth and the §4.3.1 sample's dispatch logic reads
//     verbatim.
//  3. On auth success: bind Identity onto tracectx and the deferred
//     ToolCall, then ServeHTTP() against the SDK handler.
//  4. On auth failure: write §7 error envelope with kind=UNAUTHENTICATED
//     via writeErrorEnvelope() and let the deferred recordToolCall
//     short-circuit to its diagnostic rlog line (DECISION 3 on the
//     bead — mcp.tool_calls.org_id is FK + NOT NULL and we cannot
//     synthesize a sentinel without a schema change).
//
// Per round-2 review (L7-W2 closure) the endpoint uses a single
// //encore:api annotation so HTTP-method routing happens inside the
// function body. Encore's raw-endpoint convention is one annotation
// per function; stacked POST+GET annotations are not supported by
// the Encore parser. The conceptual `method=*` from SPEC §4.3.1's
// sample is elided in the literal annotation because the Encore
// v1.52.1 parser rejects it with E1371; the raw-endpoint default
// (per ENCORE.md) is to match every HTTP method when `method=` is
// omitted, which is identical to the `method=*` intent.
package mcp

import (
	"bytes"
	"io"
	"net/http"
	"strings"
	"time"

	"encore.app/auth"
	"encore.app/shared/tracectx"
	"encore.app/shared/ulid"
	"encore.dev/beta/errs"
	"encore.dev/rlog"
)

// bearerPrefix mirrors auth/authhandler.go's constant. We accept it
// case-insensitively (RFC 6750 says Bearer is case-insensitive) and
// reject any deviation from `Bearer <token>` with UNAUTHENTICATED.
const bearerPrefix = "Bearer "

// maxJSONRPCBodyForIDProbe caps how much of the inbound POST body we
// buffer to extract the JSON-RPC `id` field for the §7 error
// envelope on auth-failure paths. 64 KiB is generous — typical
// JSON-RPC initialize / tools/call envelopes are <2 KiB. The cap
// exists so a malicious caller cannot force unbounded memory
// allocation on the pre-auth path. The body is replayed onto the
// request before SDK delegation (we never consume it on the success
// path because auth-failure short-circuits before that).
const maxJSONRPCBodyForIDProbe = 64 * 1024

// MCPHandler is the single MCP entry point. Both POST and GET hit
// the same handler; HTTP-method dispatch happens inside the function
// body. The Go MCP SDK adapter (transport.go) owns the JSON-RPC and
// SSE framing once auth resolves.
//
// The handler mints a ULID trace_id at request entry, binds it onto
// ctx via tracectx.With, and defers recordToolCall so one
// mcp.tool_calls row is written per authenticated dispatch (SPEC
// §8.1 + §10.2 Option B). The 405 method-not-allowed path and the
// auth-failure path both bypass the audit row by design — the
// mcp.tool_calls schema requires NOT NULL org_id with a FK to
// org.organizations, and no row is meaningful before Identity
// resolves (DECISION 3 on the bead).
//
//encore:api public raw path=/mcp
func MCPHandler(w http.ResponseWriter, r *http.Request) {
	// Thin wrapper around serveMCP. The split exists because
	// Encore's parser rejects in-process references to raw API
	// endpoints (E1387/E1389), which prevents the integration test
	// suite under apps/api/shared/mcpaudittest/ from spinning up an
	// httptest.Server wrapping MCPHandler — and the encore test
	// runner does not route raw //encore:api endpoints on the
	// in-process listener (the A-5 DEVIATION recorded in
	// mcpaudittest_test.go:62-75 is still present in v1.52.1).
	// serveMCP is a plain http.HandlerFunc; the test suite calls
	// it via ServeMCPForTest (export_test_writer.go).
	serveMCP(w, r)
}

// serveMCP implements the MCP transport contract independent of
// the Encore raw-endpoint annotation. Production traffic reaches it
// via MCPHandler; the integration test suite reaches it via
// ServeMCPForTest. The behavioural contract is documented on the
// MCPHandler doc-comment above — this function is the
// implementation.
func serveMCP(w http.ResponseWriter, r *http.Request) {
	// 1. HTTP-method filter BEFORE auth. AC #3 requires
	//    PUT/PATCH/DELETE to return 405 + Allow: POST, GET
	//    REGARDLESS of Authorization header presence. If we let
	//    Encore's authhandler fire (or if we Validate first), an
	//    unauthenticated PUT would 401, contradicting the
	//    spec. Filter early and return.
	switch r.Method {
	case http.MethodPost, http.MethodGet:
		// fall through to auth + SDK dispatch
	default:
		w.Header().Set("Allow", "POST, GET")
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}

	// 2. Mint trace_id (SPEC §10.2 Option B). On the extremely
	//    rare crypto/rand failure path we log + continue with an
	//    empty trace_id; the audit row's trace_id column is
	//    nullable so the request still completes.
	traceID, err := ulid.New()
	if err != nil {
		rlog.Error("mcp: trace_id mint failed", "err", err)
	}

	// 3. Bind ctx with trace_id + service. Identity fields are
	//    populated AFTER Validate resolves the Bearer (step 5
	//    below); recordToolCall reads tracectx.TraceID(ctx) so the
	//    deferred audit row carries the same id.
	ctx := tracectx.With(r.Context(), tracectx.Fields{
		TraceID: traceID,
		Service: "mcp",
	})
	r = r.WithContext(ctx)

	// 4. Capture entry time on a monotonic clock for the deferred
	//    duration_ms computation. time.Now() carries a monotonic
	//    reading on Go 1.21+, and time.Since uses it automatically.
	start := time.Now()

	// 5. Default audit fields. Populated by the SDK dispatch layer
	//    via the same `call` local — D-2..D-6 tool handlers will
	//    set ToolName / ResultKind / ItemID / ProjectID inside
	//    their wrapper closure once the JSON-RPC method has been
	//    parsed. In D-1 no tools are registered so the SDK only
	//    responds to initialize / list_tools — both of which leave
	//    the call as "transport" with ResultKind=ok.
	call := ToolCall{
		ToolName:   "transport",
		ResultKind: ResultOK,
	}

	// 6. Deferred audit write. Re-reads `call` at defer time so the
	//    SDK dispatch layer can mutate the local `call` variable
	//    inside the request body and have the final values land on
	//    the audit row. recordToolCall short-circuits to a diagnostic
	//    rlog line when call.OrgID is empty (pre-auth path), so the
	//    405 short-circuit above never reaches it (we returned).
	defer func() {
		call.DurationMs = int(time.Since(start) / time.Millisecond)
		recordToolCall(ctx, call)
	}()

	// 7. Parse Bearer + Validate. On any failure write a §7 error
	//    envelope (kind=UNAUTHENTICATED) and return — the deferred
	//    recordToolCall sees an empty OrgID and short-circuits per
	//    DECISION 3.
	authzHeader := r.Header.Get("Authorization")
	if authzHeader == "" {
		writeUnauthenticated(w, r, "missing Authorization header")
		return
	}
	token, ok := parseBearer(authzHeader)
	if !ok {
		writeUnauthenticated(w, r, "Authorization header must be \"Bearer <token>\"")
		return
	}

	// Bearer parsed — the §4.3.2 hot path runs against
	// auth.Validate. Direct in-package Go call (DECISION 1 shape
	// (b)). auth.Validate returns Unauthenticated on any miss /
	// revoke / expiry / HMAC failure; we collapse all of those to
	// the same §7 UNAUTHENTICATED kind (no information leak).
	resp, err := auth.Validate(ctx, &auth.ValidateRequest{
		Token:     token,
		TokenKind: "api_key",
	})
	if err != nil {
		// Unauthenticated covers revoke / expiry / bad HMAC. Any
		// non-Unauthenticated error (e.g. Internal from a DB
		// outage) is logged here and the caller still sees
		// UNAUTHENTICATED — preserving the no-information-leak
		// contract. Operators see the real cause via rlog +
		// trace_id.
		code := errCodeOf(err)
		if code != errs.Unauthenticated {
			rlog.Error("mcp: auth.Validate failed with non-Unauthenticated error",
				"err", err,
				"code", code.String(),
				"trace_id", traceID,
			)
		}
		writeUnauthenticated(w, r, "invalid api key")
		return
	}

	// 8. Auth success — re-bind ctx with the resolved Identity. The
	//    Tool field stays empty for D-1 (no tool dispatch yet); the
	//    D-2..D-6 tool handlers will re-bind once they parse the
	//    JSON-RPC method name. ProjectID is set by tool handlers
	//    that resolve a project scope from input args.
	ctx = tracectx.With(ctx, tracectx.Fields{
		TraceID:   traceID,
		Service:   "mcp",
		OrgID:     resp.Identity.OrgID,
		UserID:    resp.Identity.UserID,
		AgentKind: resp.Identity.AgentKind,
	})
	r = r.WithContext(ctx)

	// Populate the deferred ToolCall with the auth-resolved fields
	// so the audit row writes the correct api_key_id + org_id.
	// ToolName/ResultKind/ItemID/ProjectID stay as defaults (or
	// get overwritten by D-2+ tool handlers inside the SDK
	// dispatch).
	call.APIKeyID = resp.APIKeyID
	call.OrgID = resp.Identity.OrgID

	// 9. Delegate to the SDK adapter. The SDK owns:
	//    - JSON-RPC parsing + dispatch (initialize, tools/list,
	//      tools/call, etc.).
	//    - Mcp-Session-Id minting on initialize and lookup on
	//      subsequent requests (SPEC §5.1).
	//    - SSE framing for GET /mcp (text/event-stream).
	//    - MCP-Protocol-Version negotiation.
	//    - KeepAlive pings every 15s on long-lived streams
	//      (transport.go's sdkKeepAliveInterval).
	//
	// The SDK writes the response via the standard http.ResponseWriter
	// — no STDOUT contamination (SPEC §11.2 NFR-12).
	sdkStreamableHandler.ServeHTTP(w, r)
}

// writeUnauthenticated is the auth-failure helper used by the four
// Bearer-rejection branches in MCPHandler. It performs the §7
// envelope write with kind=UNAUTHENTICATED, lifts the JSON-RPC id
// from the inbound POST body (when present), and short-circuits
// the caller. The body-buffering only fires on the failure path —
// the success path lets the SDK consume the original body
// unchanged.
//
// On GET requests (or when the body is empty / unreadable) we pass
// `null` as the id per JSON-RPC 2.0 §5.
func writeUnauthenticated(w http.ResponseWriter, r *http.Request, message string) {
	id := jsonRPCNullID()
	if r.Method == http.MethodPost && r.Body != nil {
		// Bound the read; the spec-conformant body is small but a
		// malicious caller could push GiB. The cap balances
		// faithful id echo against pre-auth memory cost.
		body, err := io.ReadAll(io.LimitReader(r.Body, maxJSONRPCBodyForIDProbe))
		if err == nil {
			id = parseJSONRPCID(body)
		}
		// Replay the body for any downstream consumer (no-op on
		// the auth-failure path because we return immediately
		// after writing the envelope, but cheap to do).
		_ = r.Body.Close()
		r.Body = io.NopCloser(bytes.NewReader(body))
	}
	writeErrorEnvelope(w, r, id, envelopeKindUnauthenticated, message, nil)
}

// jsonRPCNullID returns the literal JSON `null` value as a
// RawMessage. Used when the inbound request did not carry a
// JSON-RPC id (e.g. GET requests, malformed POST bodies). Kept as a
// helper rather than a package var so each caller gets a fresh
// slice and cannot accidentally mutate a shared backing array.
func jsonRPCNullID() []byte {
	return []byte("null")
}

// errCodeOf extracts the errs.ErrCode from err by type assertion on
// *errs.Error. Mirrors auth/authhandler_test.go's errCode helper.
// Returns errs.OK for nil and errs.Unknown for any non-*errs.Error.
//
// Why not call errs.Code(err)? In encore.dev v1.52.1 the top-level
// errs.Code helper panics outside the Encore CLI (it forwards to a
// runtime stub). MCPHandler runs under the Encore CLI in production
// so errs.Code would work there, but the package-load contract per
// SPEC §3.1 + bead unblock-xuk requires every package to load
// cleanly under plain `go test` — so we avoid the runtime stub at
// the package level. Same workaround pattern as
// auth/authhandler_test.go.
func errCodeOf(err error) errs.ErrCode {
	if err == nil {
		return errs.OK
	}
	var e *errs.Error
	for cur := err; cur != nil; {
		if x, ok := cur.(*errs.Error); ok {
			e = x
			break
		}
		type unwrapper interface{ Unwrap() error }
		u, ok := cur.(unwrapper)
		if !ok {
			break
		}
		cur = u.Unwrap()
	}
	if e == nil {
		return errs.Unknown
	}
	return e.Code
}

// parseBearer extracts the token portion of an `Authorization:
// Bearer <token>` header. Mirrors auth/authhandler.go's parseBearer
// but lives here as an unexported helper because exporting the auth
// version would surface an internal helper across the service
// boundary for no gain (the implementation is six lines).
//
// Returns ("", false) on any deviation from the expected shape.
// Accepts case-insensitive `Bearer` for resilience (RFC 6750
// §2.1).
func parseBearer(authzHeader string) (string, bool) {
	if len(authzHeader) <= len(bearerPrefix) {
		return "", false
	}
	if !strings.EqualFold(authzHeader[:len(bearerPrefix)], bearerPrefix) {
		return "", false
	}
	tok := authzHeader[len(bearerPrefix):]
	if tok == "" || tok != strings.TrimSpace(tok) {
		return "", false
	}
	return tok, true
}
