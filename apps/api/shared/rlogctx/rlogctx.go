// Package rlogctx adapts the Encore-free trace-context carrier
// (apps/api/shared/tracectx) onto encore.dev/rlog. It produces an
// rlog.Ctx pre-bound with the canonical structured fields per
// SPEC §8.2 so every log line emitted via the returned Ctx carries
// the same audit/business correlation identifiers in JSON-Lines
// format on STDERR.
//
// Why this is a separate package from tracectx (and not a method on
// Fields): tracectx is intentionally Encore-free so it can be
// imported from non-Encore consumers (plain `go test`, the seeder
// CLI, leaf shared/* packages). encore.dev/rlog panics at package
// load outside the Encore CLI, so this binder lives in a separate
// sub-package that only Encore-runtime call sites import.
//
// SPEC anchors:
//
//   - §8.2 — required structured fields: trace_id, org_id,
//     project_id, user_id, agent_kind, tool, service.
//   - §10.2 Option B — trace_id is the audit/business correlation
//     id; Encore's runtime req.Trace.TraceID is NOT emitted on
//     application log lines and is observability-only.
//
// Forward-contract note (Sherlock investigation, Risk LOW): existing
// rlog call sites in auth/org that pre-date A-5 (e.g. auth.go:177
// "auth: api_key lookup failed") do NOT carry trace_id today. SPEC
// §8.2 reads naturally as "required for MCP-path logs" because
// `tool` is only meaningful on MCP-path logs (line 1851: "tool —
// tool name on MCP-path logs"). This package is therefore a
// forward contract for NEW log call sites (MCP tool handlers,
// recordToolCall, the cascade subscriber); legacy BFF/seeder call
// sites stay as-is for P01 and may be retrofitted in a follow-up
// bead if needed.
package rlogctx

import (
	"context"

	"encore.app/shared/tracectx"
	"encore.dev/rlog"
)

// canonical log-field keys. The literal spellings are normative
// per SPEC §8.2 (snake_case, exact names).
const (
	keyTraceID   = "trace_id"
	keyOrgID     = "org_id"
	keyProjectID = "project_id"
	keyUserID    = "user_id"
	keyAgentKind = "agent_kind"
	keyTool      = "tool"
	keyService   = "service"
)

// Bind returns an rlog.Ctx pre-bound with the canonical structured
// fields read from ctx via tracectx.From. Empty-string fields are
// elided so log lines never carry blank values for unknown
// identifiers. Always safe to call: a ctx with no bound Fields
// produces an rlog.Ctx with no extra fields (Bind never panics).
//
// Hot path: one allocation per call (the variadic []any captured by
// rlog.With). Callers that emit multiple log lines for a single
// request should bind once and re-use the returned Ctx.
//
// Usage:
//
//	lc := rlogctx.Bind(ctx)
//	lc.Info("claim accepted", "item_id", id)
//	lc.Error("claim rejected", "kind", "ALREADY_CLAIMED")
//
// Returns rlog.With() (the zero Ctx pre-bound to nothing) when ctx
// has no Fields binding — callers always receive a usable Ctx and
// never need a nil-check.
func Bind(ctx context.Context) rlog.Ctx {
	f, ok := tracectx.From(ctx)
	if !ok {
		return rlog.With()
	}
	return rlog.With(fieldsToKV(f)...)
}

// fieldsToKV flattens a Fields struct into the variadic
// key/value pairs accepted by rlog.With. Zero values are elided
// per the package contract. The output ordering is stable
// (trace_id first, then identity scope, then call surface) so log
// readers and grep heuristics see a predictable column order.
func fieldsToKV(f tracectx.Fields) []any {
	kv := make([]any, 0, 14) // 7 fields × 2 entries.
	if f.TraceID != "" {
		kv = append(kv, keyTraceID, f.TraceID)
	}
	if f.OrgID != "" {
		kv = append(kv, keyOrgID, f.OrgID)
	}
	if f.ProjectID != "" {
		kv = append(kv, keyProjectID, f.ProjectID)
	}
	if f.UserID != "" {
		kv = append(kv, keyUserID, f.UserID)
	}
	if f.AgentKind != "" {
		kv = append(kv, keyAgentKind, f.AgentKind)
	}
	if f.Tool != "" {
		kv = append(kv, keyTool, f.Tool)
	}
	if f.Service != "" {
		kv = append(kv, keyService, f.Service)
	}
	return kv
}
