// Package mcp owns the public Streamable HTTP MCP endpoint at /mcp and the
// 14 P01 tool handlers. See SPEC §4.3 (transport, auth, hot path) and §6.2
// (tool catalogue).
//
// In P01 task A-1 this package only declares the //encore:api raw endpoint
// skeleton so Encore recognises mcp as a service. The handler currently
// rejects every method with 405 (Allow: POST, GET) — POST/GET dispatch
// into the Go MCP SDK lands in D-1 (unblock-tv8.16) per SPEC §4.3.1.
//
// Per round-2 review (L7-W2 closure) the endpoint uses a single
// //encore:api annotation with `method=*` so HTTP-method routing happens
// inside the function body. Encore's raw-endpoint convention is one
// annotation per function; stacked POST+GET annotations are not supported
// by the Encore parser.
package mcp

import "net/http"

// MCPHandler is the single MCP entry point. Both POST and GET hit the same
// handler; HTTP-method dispatch happens inside the function body. The Go
// MCP SDK delegates land in task D-1 — until then the skeleton replies 405
// for every request (the auth handler in §4.3.3 will short-circuit before
// this body runs once Bearer auth wires up in B-1/D-1).
//
//encore:api public raw path=/mcp
func MCPHandler(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case http.MethodPost, http.MethodGet:
		// TODO(D-1, unblock-tv8.16): delegate to Go MCP SDK Streamable
		// HTTP transport adapter (POST = client→server, GET = server-
		// initiated SSE). Until the SDK is pinned, fall through to 405
		// so callers get a deterministic skeleton response.
		fallthrough
	default:
		w.Header().Set("Allow", "POST, GET")
		http.Error(w, "mcp: not implemented in P01 A-1 skeleton", http.StatusMethodNotAllowed)
	}
}
