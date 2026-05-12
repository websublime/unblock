// Package mcp owns the public Streamable HTTP MCP endpoint at /mcp and
// the 14 P01 tool handlers. See SPEC §4.3 (transport, auth, hot path)
// and §6.2 (tool catalogue).
//
// In P01 task A-1 this package only declared the //encore:api raw
// endpoint skeleton so Encore recognised mcp as a service. The
// handler still rejects every method with 405 (Allow: POST, GET)
// because the Go MCP SDK delegate lands in D-1 (unblock-tv8.16).
//
// Bead A-5 (unblock-tv8.5) layers the cross-cutting tracing scaffold
// on top of that skeleton:
//
//  1. Mint a ULID trace_id at request entry (SPEC §10.2 Option B).
//  2. Bind the trace_id + (later, in D-1) the resolved Identity onto
//     ctx via tracectx.With so every downstream callee — the auth
//     handler, the SDK delegate, recordToolCall, every Encore
//     private RPC, the cascade publisher — sees the same id.
//  3. Defer recordToolCall so every observed request produces exactly
//     one mcp.tool_calls audit row (SPEC §8.1), even on the 405
//     skeleton path. The deferred call captures the elapsed time
//     via time.Since on a monotonic clock and writes a sentinel
//     row with result_kind='error' / error_code='UNIMPLEMENTED'
//     until D-1 plugs the SDK in.
//
// Per round-2 review (L7-W2 closure) the endpoint uses a single
// //encore:api annotation with `method=*` so HTTP-method routing
// happens inside the function body. Encore's raw-endpoint convention
// is one annotation per function; stacked POST+GET annotations are
// not supported by the Encore parser.
package mcp

import (
	"net/http"
	"time"

	"encore.app/shared/tracectx"
	"encore.app/shared/ulid"
	"encore.dev/rlog"
)

// MCPHandler is the single MCP entry point. Both POST and GET hit
// the same handler; HTTP-method dispatch happens inside the function
// body. The Go MCP SDK delegates land in task D-1 — until then the
// skeleton replies 405 for every request (the auth handler in
// §4.3.3 will short-circuit before this body runs once Bearer auth
// wires up in B-1/D-1).
//
// The handler mints a ULID trace_id at request entry, binds it onto
// ctx via tracectx.With, and defers recordToolCall so one
// mcp.tool_calls row is written per observed request (SPEC §8.1 +
// §10.2 Option B).
//
//encore:api public raw path=/mcp
func MCPHandler(w http.ResponseWriter, r *http.Request) {
	// 1. Mint trace_id (SPEC §10.2 Option B). On the extremely
	//    rare crypto/rand failure path we log + write the response
	//    with an empty trace_id; the audit row's trace_id column
	//    is nullable so this degrades gracefully rather than
	//    failing the request.
	traceID, err := ulid.New()
	if err != nil {
		rlog.Error("mcp: trace_id mint failed", "err", err)
	}

	// 2. Bind ctx with the trace_id. Subsequent code paths
	//    (auth handler, tool dispatch, recordToolCall, the
	//    cascade publisher) read tracectx.From(ctx) to pull the
	//    same id. The Service field anchors the structured log
	//    field set per SPEC §8.2; OrgID/ProjectID/UserID/AgentKind/Tool
	//    get filled in by the tool-dispatch layer in D-1 once the
	//    auth handler has resolved Identity and the JSON-RPC
	//    method has been parsed.
	ctx := tracectx.With(r.Context(), tracectx.Fields{
		TraceID: traceID,
		Service: "mcp",
	})
	r = r.WithContext(ctx)

	// 3. Capture entry time on a monotonic clock for the deferred
	//    duration_ms computation. time.Now() carries a monotonic
	//    reading on Go 1.21+, and time.Since uses it automatically.
	start := time.Now()

	// 4. Default audit fields for the 405 skeleton path. D-1 will
	//    overwrite ToolName / ResultKind / ErrorCode / etc. inside
	//    the SDK dispatch layer before the defer fires.
	call := ToolCall{
		ToolName:   "_skeleton",
		ResultKind: ResultError,
		ErrorCode:  "UNIMPLEMENTED",
	}

	// 5. Deferred audit write. Re-reads `call` at defer time so D-1
	//    (and tests that wrap MCPHandler) can mutate the local
	//    `call` variable inside the request body and have the
	//    final values land on the audit row.
	defer func() {
		call.DurationMs = int(time.Since(start) / time.Millisecond)
		recordToolCall(ctx, call)
	}()

	switch r.Method {
	case http.MethodPost, http.MethodGet:
		// TODO(D-1, unblock-tv8.16): delegate to Go MCP SDK
		// Streamable HTTP transport adapter (POST = client→server,
		// GET = server-initiated SSE). Until the SDK is pinned,
		// fall through to 405 so callers get a deterministic
		// skeleton response.
		fallthrough
	default:
		w.Header().Set("Allow", "POST, GET")
		http.Error(w, "mcp: not implemented in P01 A-1 skeleton", http.StatusMethodNotAllowed)
	}
}
