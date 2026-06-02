// recordtoolcall.go is the audit writer for the mcp.tool_calls
// table. Every MCP request dispatched through MCPHandler invokes
// recordToolCall in a deferred epilogue so a single row lands per
// request — even on the 405 skeleton path that returns before any
// tool is dispatched.
//
// SPEC anchors:
//
//   - §8.1 — locked column set:
//     (id, api_key_id, org_id, project_id, item_id, tool_name,
//     arguments, result_kind, rejection_reason, error_code,
//     warning_codes, duration_ms, trace_id, called_at). The
//     warning_codes jsonb column is the §8.1.1 additive audit
//     channel for the §7.1 success-side warnings (added
//     unblock-tv8.63); a nil / empty WarningCodes slice serialises
//     to the '[]'::jsonb default.
//   - §10.2 Option B — trace_id is the ULID minted by MCPHandler
//     at request entry, propagated via context.Context and pulled
//     here via tracectx.TraceID(ctx). The column type is `text`
//     (DDL frozen in apps/api/db/migrations/0070_mcp.up.sql:64)
//     and accepts the ULID verbatim.
//
// Failure semantics (Sherlock investigation, RISK MEDIUM): the
// insert is fire-and-forget. If the DB write fails (network blip,
// pool exhaustion, FK violation against a deleted api_key), we log
// at error level with the trace_id structured field and return —
// we do NOT propagate the error to the caller because that would
// mask the underlying tool result. The tool itself already
// committed its work (or returned its own error); the audit row
// is supplementary forensic data and its absence is acceptable.
// Encore's rlog stack records the error; the operator alert tier
// (P03) will surface it.
//
// Duration semantics: callers pass elapsed time computed via
// time.Since(start) on a monotonic clock at the MCPHandler entry.
// duration_ms is the integer millisecond value; sub-millisecond
// requests round down to zero (acceptable — the column granularity
// is intentional, not a bug).

package mcp

import (
	"context"
	"encoding/json"
	"sync"

	"encore.app/shared/tracectx"
	"encore.app/shared/ulid"
	"encore.dev/rlog"
)

// ResultKind enumerates the values accepted by the
// mcp.tool_calls.result_kind column. The DDL enforces this set via
// the tool_calls_result_chk CHECK constraint
// (apps/api/db/migrations/0070_mcp.up.sql:67) — keep these
// constants in sync.
type ResultKind string

const (
	// ResultOK means the tool dispatched successfully and
	// returned a result envelope per SPEC §6.x.
	ResultOK ResultKind = "ok"

	// ResultRejected means the tool ran but rejected the call
	// with a structural precondition failure (P01) or a BLOCK
	// condition (P02+). RejectionReason carries the canonical
	// reason name (e.g. "claimed_by_id" missing). Maps to the
	// PRECONDITION_NOT_MET kind in the §7 error envelope.
	ResultRejected ResultKind = "rejected"

	// ResultError means the tool failed for any reason other than
	// a structural rejection (validation, auth, internal). The
	// ErrorCode field carries the §7 error envelope `kind` value
	// (UNAUTHENTICATED, FORBIDDEN, VALIDATION, INTERNAL, …).
	ResultError ResultKind = "error"
)

// ToolCall is the input record for recordToolCall — the locked
// per-request audit payload per SPEC §8.1.
//
// Zero-valued nullable fields are written as SQL NULL where the
// column permits it (api_key_id, project_id, item_id,
// rejection_reason, error_code, trace_id). The required columns
// (org_id, tool_name, result_kind, duration_ms) MUST be set
// before calling recordToolCall — the DB NOT NULL + CHECK
// constraints will reject any row missing them.
//
// id is NOT a ToolCall field: recordToolCall generates the ULID
// internally so callers cannot accidentally reuse one across rows
// (which would silently violate the PRIMARY KEY).
type ToolCall struct {
	// APIKeyID is the mcp.api_keys.id ULID for the caller's
	// Bearer key. Pulled from auth.Data() once Validate has
	// resolved the key. Empty string when the call rejected at
	// the //encore:authhandler (UNAUTHENTICATED) before any
	// key id was determined — written as NULL.
	APIKeyID string

	// OrgID is the org-scope of the call. Required (the
	// mcp.tool_calls.org_id column is NOT NULL). Pulled from
	// Identity.OrgID once the auth handler resolves; on auth
	// failures the caller writes a sentinel placeholder (see
	// recordToolCall callers in mcp.go).
	OrgID string

	// ProjectID is the project scope when the tool resolves
	// one from its input args (e.g. ready, list). Empty
	// string written as NULL.
	ProjectID string

	// ItemID is the workitem the tool targeted (claim, close,
	// set_state, add_dependency, …). Empty string written as
	// NULL for tools that do not target a single item (prime,
	// ready, list, search, get_trail).
	ItemID string

	// ToolName is the canonical tool identifier per SPEC §6.2
	// (prime, ready, claim, set_state, append_comment, close,
	// add_dependency, remove_dependency, list, search,
	// get_trail, create_item, create_milestone, …). Required.
	ToolName string

	// Arguments is the JSON-encoded input arguments envelope.
	// Pre-marshalled by the caller so this writer is
	// dependency-free; a nil / empty value defaults to the
	// literal `{}` JSON object (matching the DDL DEFAULT).
	// Sensitive fields MUST be stripped or redacted by the
	// caller before reaching recordToolCall.
	Arguments json.RawMessage

	// ResultKind classifies the outcome (see ResultKind).
	// Required.
	ResultKind ResultKind

	// RejectionReason is the canonical precondition / BLOCK
	// reason name when ResultKind is "rejected". Empty for
	// "ok" and "error".
	RejectionReason string

	// ErrorCode is the §7 envelope `kind` machine code when
	// ResultKind is "error" (UNAUTHENTICATED, FORBIDDEN,
	// NOT_FOUND, VALIDATION, CONFLICT, INTERNAL, …).
	ErrorCode string

	// WarningCodes carries the §7.1 success-side warning `code`
	// strings present on the tool's success-result warnings[]
	// (§8.1.1 mcp.tool_calls.warning_codes). The only P01/P02
	// producer is set_state's intent_comment_dropped on the
	// AppendComment-failure branch; result_kind STAYS "ok" on that
	// path. A nil / empty slice serialises to the jsonb default '[]'
	// (see recordToolCall). Mirrors the Arguments jsonb handling.
	WarningCodes []string

	// DurationMs is the elapsed wall-clock millisecond count
	// from MCPHandler entry to the deferred call.
	DurationMs int
}

// recordToolCall inserts one row into mcp.tool_calls per the locked
// §8.1 contract. Fire-and-forget: any insert error is logged with
// the trace_id structured field and swallowed (see the file-level
// failure-semantics doc-comment).
//
// trace_id is read from ctx via tracectx.TraceID — callers do not
// pass it as a ToolCall field because the ctx binding is the
// canonical source per SPEC §10.2 Option B and duplicating it on
// the struct invites drift (e.g. caller mints ULID #1, ctx carries
// ULID #2). Passing ctx-bound + struct-set values would also
// double-emit on the audit row.
func recordToolCall(ctx context.Context, call ToolCall) {
	// Pre-auth / skeleton-path guard: the mcp.tool_calls table
	// requires NOT NULL org_id with a FK to org.organizations.
	// Calls that reach the deferred audit before the auth handler
	// has resolved Identity (the P01 405 skeleton path, future
	// UNAUTHENTICATED early-returns) have no OrgID; inserting
	// without one would violate the constraint and the row would
	// be dropped silently by the fire-and-forget error path
	// anyway. Skip the insert and emit a structured log entry
	// instead so the trace_id is still observable in rlog —
	// satisfying the §10.2 "trace_id visible in JSON-Lines output"
	// contract — without breaking the §8.1 audit invariant for
	// authenticated dispatches.
	//
	// Once D-1 lands the SDK + B-1's auth handler is on the hot
	// path, every dispatch reaches this writer with a populated
	// OrgID and this branch becomes effectively dead.
	if call.OrgID == "" {
		rlog.Info("mcp: recordToolCall: pre-auth skeleton path, skipping audit row",
			"trace_id", tracectx.TraceID(ctx),
			"tool", call.ToolName,
			"result_kind", string(call.ResultKind),
			"error_code", call.ErrorCode,
		)
		return
	}

	// Defense-in-depth: if BindDB has not run yet (impossible
	// at runtime — Encore's bootstrap guarantees init order —
	// but cheap insurance for unit tests that may exercise this
	// writer with a fake), log and return without touching the
	// handle.
	if db == nil {
		rlog.Error("mcp: recordToolCall: db handle not bound",
			"trace_id", tracectx.TraceID(ctx),
			"tool", call.ToolName,
		)
		return
	}

	id, err := ulid.New()
	if err != nil {
		// crypto/rand failure: extremely rare. Log and return —
		// we cannot insert without a PK.
		rlog.Error("mcp: recordToolCall: ulid mint failed",
			"err", err,
			"trace_id", tracectx.TraceID(ctx),
			"tool", call.ToolName,
		)
		return
	}

	// Pull trace_id from ctx. Empty string is acceptable (column
	// is nullable) — the only call path that produces an empty
	// trace_id is a code path that bypasses MCPHandler entry,
	// which by construction does not happen in P01.
	traceID := tracectx.TraceID(ctx)

	// Default Arguments to `{}` jsonb when the caller did not
	// supply a body. Matches the DDL DEFAULT but explicit here
	// so the INSERT is a single shape and Postgres does not
	// need to consult the default.
	args := call.Arguments
	if len(args) == 0 {
		args = json.RawMessage(`{}`)
	}

	// Marshal WarningCodes ([]string) to a jsonb array. A nil /
	// empty slice MUST serialise to the literal `[]` (the §8.1.1
	// column is NOT NULL DEFAULT '[]'::jsonb) — json.Marshal(nil
	// []string) yields "null", so collapse that to "[]" explicitly,
	// mirroring the Arguments `{}` default above. A marshal error is
	// effectively impossible for a []string but is handled defensively
	// by falling back to the empty array so the audit row still lands.
	warningCodes := []byte(`[]`)
	if len(call.WarningCodes) > 0 {
		if encoded, marshalErr := json.Marshal(call.WarningCodes); marshalErr == nil {
			warningCodes = encoded
		} else {
			rlog.Error("mcp: recordToolCall: warning_codes marshal failed; defaulting to []",
				"err", marshalErr,
				"trace_id", tracectx.TraceID(ctx),
				"tool", call.ToolName,
			)
		}
	}

	// Nullable string columns: collapse empties to (*string)(nil)
	// so the SQL driver emits NULL. encore.dev/storage/sqldb
	// forwards (*string)(nil) as NULL on the wire.
	apiKeyID := nullable(call.APIKeyID)
	projectID := nullable(call.ProjectID)
	itemID := nullable(call.ItemID)
	rejectionReason := nullable(call.RejectionReason)
	errorCode := nullable(call.ErrorCode)
	traceIDArg := nullable(traceID)

	_, execErr := db.Exec(ctx, `
		INSERT INTO mcp.tool_calls
			(id, api_key_id, org_id, project_id, item_id,
			 tool_name, arguments, result_kind, rejection_reason,
			 error_code, warning_codes, duration_ms, trace_id)
		VALUES
			($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
	`,
		id,
		apiKeyID,
		call.OrgID,
		projectID,
		itemID,
		call.ToolName,
		[]byte(args),
		string(call.ResultKind),
		rejectionReason,
		errorCode,
		warningCodes,
		call.DurationMs,
		traceIDArg,
	)
	if execErr != nil {
		// Fire-and-forget: do NOT propagate. Log with the full
		// audit context so a downstream operator can correlate.
		rlog.Error("mcp: recordToolCall: insert failed",
			"err", execErr,
			"trace_id", traceID,
			"tool", call.ToolName,
			"org_id", call.OrgID,
			"result_kind", string(call.ResultKind),
		)
		return
	}
}

// nullable converts an empty string to a typed nil pointer so the
// SQL driver writes NULL; non-empty strings pass through as their
// own pointer. encore.dev/storage/sqldb forwards (*string)(nil) as
// NULL on the wire.
func nullable(s string) *string {
	if s == "" {
		return nil
	}
	return &s
}

// requestState is the per-request data block tool handlers need to
// reach from inside the SDK's session-based dispatch. It carries:
//
//   - Call: pointer to the deferred audit record so handlers can
//     mutate ToolName / ResultKind / ItemID / ProjectID.
//   - TraceID: ULID minted at request entry; the §7 envelope writer
//     and the deferred recordToolCall both stamp this verbatim.
//   - Identity: the auth.Identity{OrgID, UserID, AgentKind} resolved
//     from the Bearer key — passed by value so the handler does NOT
//     accidentally hold a pointer into a per-request stack slot.
//
// Why a process-wide registry instead of context.Context value
// propagation: the Go MCP SDK uses stateful sessions where the tool
// handler runs under the CONNECT-time context (i.e. the original
// initialize request's ctx — streamable.go:488 "Pass req.Context()
// here"), NOT under the subsequent tools/call request's ctx. Any
// ctx-bound state therefore only works for the initialize request
// and is stale for every subsequent dispatch. The SDK does plumb
// each request's HTTP headers through RequestExtra
// (streamable.go:1163-1166), which is the canonical per-request
// channel. We use the trace_id ULID as the registry key (injected
// as an X-Unblock-Trace-Id header by serveMCP).
//
// Cleanup: serveMCP MUST defer release(); the map otherwise leaks
// one entry per authenticated request.

// requestState bundles the per-request data tool handlers need.
type requestState struct {
	Call     *ToolCall
	TraceID  string
	Identity requestIdentity
}

// requestIdentity is the narrow tuple of Identity fields the
// handlers consume — mirrors auth.Identity but lives in this
// package so the recordtoolcall.go primitives stay self-contained
// (importing auth would form a dependency edge purely for the
// shape of a value type).
type requestIdentity struct {
	UserID    string
	OrgID     string
	AgentKind string
}

//nolint:gochecknoglobals // process-wide lookup table by design.
var requestStateRegistry sync.Map // map[string]*requestState

// traceIDHeader is the canonical HTTP header serveMCP uses to
// propagate the request trace_id into the SDK handler dispatch.
// The header is internal to the MCP transport boundary — clients
// MUST NOT set it (the value is overwritten in serveMCP) and the
// public observability surface uses the §7 envelope / log-line
// fields. Named X-Unblock- to follow the existing X-Unblock-BFF-
// Origin convention.
const traceIDHeader = "X-Unblock-Trace-Id"

// registerRequestState stores state under traceID. Returns a
// release func the caller MUST invoke (typically via defer) to
// clear the entry once the request has completed.
func registerRequestState(traceID string, state *requestState) (release func()) {
	if traceID == "" || state == nil {
		return func() {}
	}
	requestStateRegistry.Store(traceID, state)
	return func() { requestStateRegistry.Delete(traceID) }
}

// requestStateFromHeaders returns the *requestState registered
// under the trace_id carried in the headers. Returns nil when no
// header is set or no entry is registered (the latter signals a
// handler invoked outside serveMCP — unit tests, future stdio
// transport — and the handler degrades gracefully).
func requestStateFromHeaders(headers map[string][]string) *requestState {
	if headers == nil {
		return nil
	}
	vals, ok := headers[traceIDHeader]
	if !ok {
		// HTTP headers are case-insensitive but Go's http.Header
		// normalises via textproto.CanonicalMIMEHeaderKey. The map
		// handed via RequestExtra is the http.Header itself so this
		// fallback should not fire — kept defensively.
		vals = headers["X-Unblock-Trace-Id"]
	}
	if len(vals) == 0 {
		return nil
	}
	v, _ := requestStateRegistry.Load(vals[0])
	if v == nil {
		return nil
	}
	return v.(*requestState)
}
