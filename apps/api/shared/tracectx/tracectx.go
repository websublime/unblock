// Package tracectx is the canonical context.Context carrier for the
// per-request ULID trace_id minted at MCP entry. It also carries the
// auxiliary structured-log fields (org_id, project_id, user_id,
// agent_kind, tool, service) defined by SPEC §8.2 so any callee can
// reconstruct the canonical log-field set from ctx alone.
//
// SPEC anchors:
//
//   - §10.2 Option B (locked round-5, 2026-05-12) — the trace_id is
//     minted by the mcp raw endpoint, propagated via context.Context
//     only, and re-emitted on every log line, on mcp.tool_calls.trace_id,
//     in the JSON-RPC error envelope (§7), and embedded in the
//     CascadeRequested Pub/Sub payload (§6.3.1). No
//     X-Unblock-Trace-Id header is set or required — Encore's
//     generated client carries context.Context across private RPCs
//     for free, and Pub/Sub publishers explicitly copy TraceID from
//     ctx into the message payload at publish time.
//   - §8.2 — required structured log fields:
//     trace_id, org_id, project_id, user_id, agent_kind, tool, service.
//
// Why this package is Encore-free (mirrors apps/api/shared/ulid/):
//
//	The tracectx primitives are used from MCP tool handlers,
//	cascade publishers, the recordToolCall writer, and tests. Some
//	of those callers run under plain `go test` (auxiliary tests
//	under shared/* and pure-value tests under <service>/types/),
//	so this package MUST stay importable without booting the
//	encore runtime. It is a pure value package: zero encore.dev/*
//	imports, no //encore:api endpoints, no infrastructure
//	resources. Consumers that need the rlog binding wrap this
//	package with apps/api/shared/rlogctx/.
//
// Constraints (DO NOT VIOLATE):
//
//   - MUST NOT import any encore.dev/* package, directly or
//     transitively.
//   - MUST NOT declare //encore:api endpoints, //encore:service
//     annotations, or infrastructure resources.
//   - The trace_id ULID is the canonical audit/business correlation
//     id; Encore's runtime req.Trace.TraceID is observability-only
//     and is not surfaced through this package (§10.2 last
//     paragraph).
package tracectx

import "context"

// Fields is the canonical structured-log field set bound onto a
// request's context.Context. Every field is optional: zero values
// signal "not known yet" and are elided by the rlogctx binder
// (apps/api/shared/rlogctx/) so log lines never carry empty strings
// for unknown identifiers.
//
// Field semantics (SPEC §8.2 + §10.2):
//
//   - TraceID:   ULID minted by the mcp raw endpoint at request
//     entry. The only mandatory field (every Fields value retrieved
//     after MCPHandler entry carries a non-empty TraceID).
//   - OrgID:     ULID — populated after the //encore:authhandler
//     resolves Identity (auth.Data().Identity.OrgID).
//   - ProjectID: ULID — populated when the tool handler resolves
//     the project scope (typically from input args).
//   - UserID:    ULID — populated from Identity.UserID.
//   - AgentKind: AgentKind enum value (claude-code, copilot, cursor,
//     codex, aider, custom) for API-key callers; empty for human
//     sessions.
//   - Tool:      tool name (prime, ready, claim, …) — set by the
//     MCP tool dispatch layer once the JSON-RPC method has been
//     parsed.
//   - Service:   Encore service emitting the log line. Populated by
//     handlers that re-bind the field set across an Encore service
//     boundary so cross-service log correlation works.
type Fields struct {
	TraceID   string
	OrgID     string
	ProjectID string
	UserID    string
	AgentKind string
	Tool      string
	Service   string
}

// ctxKey is the unexported context.Context key type used by every
// With/From pair in this package. Using a named, package-private
// type prevents collisions with other context.Value consumers per
// the standard library's documented contract.
type ctxKey struct{}

// With returns ctx augmented with f as the canonical trace fields.
// Subsequent calls to From on the returned context (or any context
// derived from it) recover the same Fields value verbatim. A nil
// ctx is treated as context.Background().
//
// With does NOT merge with previously bound fields — it replaces.
// Callers that want to extend the field set should first call From,
// mutate the returned struct, and call With with the merged value.
// This mirrors the rlog.With shape (which always produces a fresh
// Ctx rather than mutating an existing one).
func With(ctx context.Context, f Fields) context.Context {
	if ctx == nil {
		ctx = context.Background()
	}
	return context.WithValue(ctx, ctxKey{}, f)
}

// From returns the Fields value bound on ctx by a prior call to
// With, or the zero Fields{} if no value is bound. The second
// return reports whether a binding was present (false signals
// "this code path was reached without going through MCPHandler" —
// expected for non-MCP private-RPC test paths and for the seeder
// CLI).
//
// A nil ctx returns (Fields{}, false).
func From(ctx context.Context) (Fields, bool) {
	if ctx == nil {
		return Fields{}, false
	}
	v, ok := ctx.Value(ctxKey{}).(Fields)
	if !ok {
		return Fields{}, false
	}
	return v, true
}

// TraceID is the most common From consumer: callers that only need
// the trace_id ULID (e.g. recordToolCall, the CascadeRequested
// publisher, the §7 error envelope writer) call TraceID(ctx)
// instead of unpacking the full Fields struct. Returns "" if no
// trace id is bound.
func TraceID(ctx context.Context) string {
	f, _ := From(ctx)
	return f.TraceID
}
