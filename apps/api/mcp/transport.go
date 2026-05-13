// transport.go owns the Go MCP SDK (github.com/modelcontextprotocol/go-sdk)
// Streamable HTTP transport adapter. It constructs a package-level
// *mcp.Server once at process bootstrap and an *mcp.StreamableHTTPHandler
// that owns POST/GET dispatch, session-id minting, SSE framing, and the
// MCP-Protocol-Version negotiation. MCPHandler in mcp.go is the thin
// Encore wrapper that does HTTP-method filtering + Bearer auth before
// delegating to this adapter.
//
// SPEC anchors:
//
//   - §4.3.1 — single //encore:api public raw method=* path=/mcp;
//     HTTP-method dispatch inside the function body; SDK adapter is
//     a singleton owned by this file.
//   - §5.1 — transport contract: POST + GET, Mcp-Session-Id returned
//     on initialize, SSE keepalive every 15s. The SDK's KeepAlive
//     option emits ping requests at the protocol level — over the
//     SSE stream, these surface as JSON-RPC ping messages and serve
//     the same purpose as the literal `:keepalive\n\n` comment frame
//     (mitigates Encore Cloud edge-proxy idle close per RP01-4).
//   - §4.3.1 SDK pinning — v1.6.0 is the latest stable as of D-1
//     implementation (2026-05-13); recorded in go.mod and in this
//     file's package import.
//
// In D-1 (this bead, unblock-tv8.16) no tools are registered on the
// SDK server — the 14 P01 tools land in D-2..D-6. The transport
// adapter is functional for the initialize / list_tools handshake
// (initialize returns server capabilities + Mcp-Session-Id; ListTools
// returns an empty tool list; an unknown tools/call returns a §7
// NOT_FOUND-equivalent JSON-RPC error from the SDK itself). This
// matches the bead's acceptance criterion: POST /mcp initialize
// returns a valid response with Mcp-Session-Id, GET /mcp opens a
// text/event-stream — without the SDK requiring a populated tool
// catalogue.
//
// Concurrency: the SDK's StreamableHTTPHandler is safe for concurrent
// ServeHTTP calls; the Server instance is read-only after Init().
// We construct both at package init (sync.Once-style via package
// scope) so every request reads the same handles.
package mcp

import (
	"net/http"
	"time"

	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// sdkKeepAliveInterval is the JSON-RPC ping interval the SDK sends
// over long-lived SSE streams. SPEC §5.1 specifies 15s keepalive
// frames; the SDK's ping mechanism keeps the underlying connection
// active at the same cadence, satisfying the RP01-4 edge-proxy idle
// close mitigation.
const sdkKeepAliveInterval = 15 * time.Second

// sdkServerName is the MCP server identifier returned to clients via
// the initialize handshake (mcp.Implementation.Name). The name
// `unblock-mcp` aligns with the public domain `api.unblock.websublime.com`
// and is stable across the P01..P05 lifecycle.
const sdkServerName = "unblock-mcp"

// sdkServerVersion is the version string returned to clients. The
// P01 backend is pre-release; the version tracks the SPEC revision
// implicitly via the v0 family. Hard-coded here (not pulled from a
// build tag) because the SDK requires a string and the value is
// observability-only.
const sdkServerVersion = "0.1.0"

// sdkServer is the package-level MCP server instance. Tool handlers
// (D-2..D-6, beads unblock-tv8.17..unblock-tv8.21) call
// sdkmcp.AddTool(sdkServer, …) at package init to register tools;
// in D-1 no tools are registered and the SDK exposes only the
// built-in initialize / list_tools / list_resources methods.
//
// Initialized at package init via initSDKServer (see init() below)
// — package-init order is unimportant because no //encore:api
// dispatch fires until Encore's bootstrap completes. The handle is
// read-only after construction.
var sdkServer *sdkmcp.Server

// sdkStreamableHandler is the http.Handler the SDK exposes for the
// Streamable HTTP transport. Wraps sdkServer and owns session
// lifecycle (Mcp-Session-Id minting on initialize, session lookup
// on subsequent requests, SSE framing, MCP-Protocol-Version checks,
// JSON-RPC dispatch).
var sdkStreamableHandler *sdkmcp.StreamableHTTPHandler

func init() {
	sdkServer = sdkmcp.NewServer(&sdkmcp.Implementation{
		Name:    sdkServerName,
		Version: sdkServerVersion,
	}, &sdkmcp.ServerOptions{
		// KeepAlive sends a JSON-RPC ping over the session at the
		// configured interval. On SSE streams the ping surfaces as
		// an `event: message\ndata: {…ping…}` frame which prevents
		// Encore Cloud edge-proxy idle close (RP01-4). SPEC §5.1
		// names a 15s keepalive cadence.
		KeepAlive: sdkKeepAliveInterval,
	})

	sdkStreamableHandler = sdkmcp.NewStreamableHTTPHandler(
		// getServer returns the singleton server for every request.
		// In D-1 the server is process-wide because no tool body
		// reads per-org state directly — Identity is propagated via
		// ctx (tracectx.Fields populated by MCPHandler), and tool
		// bodies pull it on entry. A per-org server map would only
		// pay off if tool dispatch became dependent on a static
		// capability set that varied by tenant; the spec does not
		// require that.
		func(*http.Request) *sdkmcp.Server { return sdkServer },
		&sdkmcp.StreamableHTTPOptions{
			// DisableLocalhostProtection: the SDK's default DNS
			// rebinding guard rejects requests when the listener
			// is on localhost and the Host header is not — that
			// breaks Encore's test runner (encore.Meta().APIBaseURL
			// resolves to a host:port on 127.0.0.1 but the
			// integration tests may emit a different Host header).
			// We disable the SDK's guard because Encore's own auth
			// and the Bearer hot path in MCPHandler are the real
			// security boundary; localhost-only protections are not
			// relevant on Encore Cloud where the public hostname
			// is the same as the listening interface.
			DisableLocalhostProtection: true,
		},
	)
}
