// errmap.go translates Encore errs.* errors raised by downstream
// private RPCs (workitems.*, deps.*, org.*) into the §7 JSON-RPC
// error envelope kind + details payload returned by the MCP tool
// handlers.
//
// The MCP SDK's ToolHandlerFor accepts a plain Go error and packs it
// into the CallToolResult.Content / IsError shape automatically;
// however, the §7 contract requires a specific data.kind machine
// code + a kind-specific details map. We satisfy both by:
//
//  1. Building the envelope payload via mapError below.
//  2. Returning a *jsonrpc.Error from the tool handler with code
//     -32000 and the envelope payload as Data — the SDK forwards
//     it verbatim per the protocol contract.
//
// Returning a non-jsonrpc.Error from the tool handler would make
// the SDK wrap the error in a tool-result envelope with IsError=true
// (see server.go:340-353), which collides with the §7 transport-level
// error envelope. The structured-error path is the right one for §7.
//
// Audit-row contract: every error mapping also mutates the
// per-request *ToolCall (state.Call) to set ResultKind + ErrorCode
// + RejectionReason. This is mandatory — without it the audit
// row's result_kind defaults to "ok" and the CHECK constraint on
// the DDL would still accept it but the dashboard would
// misattribute the failure.

package mcp

import (
	"encoding/json"
	"errors"
	"fmt"
	"strings"

	"encore.dev/beta/errs"
	sdkjsonrpc "github.com/modelcontextprotocol/go-sdk/jsonrpc"
)

// mapError translates an Encore *errs.Error into a *sdkjsonrpc.Error
// carrying the §7 envelope payload. The returned error MUST be
// returned directly from a tool handler so the SDK forwards the
// payload verbatim on the JSON-RPC error response.
//
// Audit-row side-effects: this function ALSO mutates the *ToolCall
// held in the per-request state — ResultKind, ErrorCode, and (for
// PRECONDITION_NOT_MET) RejectionReason. Callers do not need to
// touch the audit record after calling mapError.
//
// tool is the MCP tool name (prime, ready, claim, create, …) used
// to populate the §7 envelope data.tool field. state may be nil
// when the handler runs outside serveMCP (unit tests); in that
// case the envelope is still produced and the audit mutation is
// a no-op.
func mapError(state *requestState, tool string, err error) error {
	if err == nil {
		return nil
	}
	envErr := classifyEnvelopeError(err)

	traceID := ""
	if state != nil {
		traceID = state.TraceID
		if call := state.Call; call != nil {
			call.ResultKind = ResultError
			call.ErrorCode = envErr.kind
			if envErr.kind == envelopeKindPreconditionNotMet {
				call.ResultKind = ResultRejected
				// Spec §8.1: rejection_reason carries the canonical
				// name of the failed precondition. Pull it from the
				// details map (set by classifyEnvelopeError for the
				// PRECONDITION path). Preference order:
				//   1. `missing` — column/argument absence cases (Close
				//      with claimed_by_id IS NULL, etc).
				//   2. `invariant` — set_state I-1..I-5 cases (added in
				//      bead unblock-tv8.21 alongside the envelope
				//      `data.invariant` surface).
				//   3. `rejection_reason` — legacy mirror, kept as a
				//      fallback for any future precondition that
				//      surfaces only this key.
				if v, ok := envErr.details["missing"].(string); ok && v != "" {
					call.RejectionReason = v
				} else if v, ok := envErr.details["invariant"].(string); ok && v != "" {
					call.RejectionReason = v
				} else if v, ok := envErr.details["rejection_reason"].(string); ok && v != "" {
					call.RejectionReason = v
				}
			}
		}
	}

	data := map[string]any{
		"kind":     envErr.kind,
		"tool":     tool,
		"trace_id": traceID,
		"details":  envErr.details,
	}

	return &sdkjsonrpc.Error{
		Code:    int64(jsonRPCErrorCode),
		Message: envErr.message,
		Data:    mustJSONRaw(data),
	}
}

// envelopeError is the intermediate triple we lift from an Encore
// errs.Error before formatting the JSON-RPC error.
type envelopeError struct {
	kind    string
	message string
	details map[string]any
}

// classifyEnvelopeError walks the Encore errs.Error metadata to
// produce a §7 envelope triple. Mapping rules per SPEC §7 table:
//
//	Unauthenticated     → UNAUTHENTICATED
//	PermissionDenied    → FORBIDDEN
//	NotFound            → NOT_FOUND
//	InvalidArgument     → VALIDATION
//	AlreadyExists       → ALREADY_CLAIMED (when Meta.reason="already_claimed")
//	                     or CONFLICT (otherwise — unique violation, dup edge)
//	FailedPrecondition  → CYCLE_DETECTED (when Meta.kind="CYCLE_DETECTED")
//	                     or PRECONDITION_NOT_MET (everything else)
//	anything else       → INTERNAL
//
// Specific Meta fields carried into details:
//
//	ALREADY_CLAIMED — winner_user_id, winner_agent, claimed_at
//	CYCLE_DETECTED  — from, to, cycle_path[] (typed slice preferred)
//	NOT_FOUND       — kind, id (when present)
//	VALIDATION      — field, reason (when present)
//	PRECONDITION    — missing, invariant, rejection_reason (when present)
//
// Unknown error types collapse to INTERNAL with empty details — the
// human-readable message is preserved verbatim.
func classifyEnvelopeError(err error) envelopeError {
	var e *errs.Error
	if !errors.As(err, &e) {
		return envelopeError{
			kind:    envelopeKindInternal,
			message: err.Error(),
			details: map[string]any{},
		}
	}

	msg := e.Message
	if msg == "" {
		msg = e.Code.String()
	}

	switch e.Code {
	case errs.Unauthenticated:
		return envelopeError{kind: envelopeKindUnauthenticated, message: msg, details: map[string]any{}}

	case errs.PermissionDenied:
		details := map[string]any{}
		if v, ok := e.Meta["resource"].(string); ok && v != "" {
			details["resource"] = v
		}
		if v, ok := e.Meta["action"].(string); ok && v != "" {
			details["action"] = v
		}
		return envelopeError{kind: envelopeKindForbidden, message: msg, details: details}

	case errs.NotFound:
		details := map[string]any{}
		if v, ok := e.Meta["kind"].(string); ok && v != "" {
			details["kind"] = v
		}
		if v, ok := e.Meta["id"].(string); ok && v != "" {
			details["id"] = v
		}
		return envelopeError{kind: envelopeKindNotFound, message: msg, details: details}

	case errs.InvalidArgument:
		details := map[string]any{}
		if v, ok := e.Meta["field"].(string); ok && v != "" {
			details["field"] = v
		}
		if v, ok := e.Meta["reason"].(string); ok && v != "" {
			details["reason"] = v
		} else {
			details["reason"] = msg
		}
		return envelopeError{kind: envelopeKindValidation, message: msg, details: details}

	case errs.AlreadyExists:
		// workitems.Claim's loser path sets Meta.reason="already_claimed"
		// (workitems.go::alreadyClaimedError). Anything else under
		// AlreadyExists is a duplicate / unique violation and maps to
		// the §7 CONFLICT kind.
		if reason, _ := e.Meta["reason"].(string); reason == "already_claimed" {
			details := map[string]any{}
			if v, ok := e.Meta["winner_user_id"].(string); ok && v != "" {
				details["winner_user_id"] = v
			}
			if v, ok := e.Meta["winner_agent"].(string); ok && v != "" {
				details["winner_agent"] = v
			}
			if v, ok := e.Meta["claimed_at"].(string); ok && v != "" {
				details["claimed_at"] = v
			}
			return envelopeError{kind: envelopeKindAlreadyClaimed, message: msg, details: details}
		}
		details := map[string]any{}
		// deps.AddEdge surfaces the unique-violation pair under "from",
		// "to", "kind" — preserve them in details.constraint so the
		// client can diagnose without guessing.
		if from, _ := e.Meta["from"].(string); from != "" {
			details["constraint"] = "dependencies_pair_uniq"
			details["from"] = from
		}
		if to, _ := e.Meta["to"].(string); to != "" {
			details["to"] = to
		}
		// workitems.CreateLabel / UpdateLabel surface the violated UNIQUE
		// index name directly under Meta["constraint"] (labels_org_name_uniq
		// / labels_project_name_uniq) — the §6.2 Tool 20 contract requires
		// data.constraint to name the index. An explicit constraint key
		// overrides the deps-inferred default above.
		if constraint, _ := e.Meta["constraint"].(string); constraint != "" {
			details["constraint"] = constraint
		}
		return envelopeError{kind: envelopeKindConflict, message: msg, details: details}

	case errs.FailedPrecondition:
		// deps.AddEdge cycle path sets Meta.kind="CYCLE_DETECTED";
		// workitems.SetStateColumns invariants set Meta.invariant=<name>
		// (PRECONDITION_NOT_MET). Discriminate on Meta.kind.
		if kind, _ := e.Meta["kind"].(string); kind == "CYCLE_DETECTED" {
			details := map[string]any{}
			if from, _ := e.Meta["from"].(string); from != "" {
				details["from"] = from
			}
			if to, _ := e.Meta["to"].(string); to != "" {
				details["to"] = to
			}
			// Cycle path: prefer the typed []string under
			// cycle_path_list; fall back to splitting the CSV form
			// under cycle_path. errs.Metadata is gob-encoded across
			// Encore RPC boundaries and gob cannot encode []interface{}
			// — the dual-encoding shape (workitems.AddEdge in
			// deps/deps.go) is the load-bearing seam here.
			if pathList, ok := e.Meta["cycle_path_list"].([]string); ok && len(pathList) > 0 {
				details["cycle_path"] = pathList
			} else if csv, ok := e.Meta["cycle_path"].(string); ok && csv != "" {
				details["cycle_path"] = strings.Split(csv, ",")
			}
			return envelopeError{kind: envelopeKindCycleDetected, message: msg, details: details}
		}
		details := map[string]any{}
		if v, ok := e.Meta["missing"].(string); ok && v != "" {
			details["missing"] = v
		}
		// SPEC §7.2 (round-16, bead unblock-tv8.71): the status-precondition
		// extension. When a tool rejects because the subject item is in the
		// WRONG Status (§6.1 enum) for the requested operation, the backing
		// RPC sets Meta["status"] (the item's CURRENT Status) and
		// Meta["required"] (the Status the operation demands). They are
		// surfaced INSIDE data.details (the locked §7 base-table shape lists
		// them in the `details` column — the §7.2 example block draws them at
		// the wrong level and is corrected in the same PR). status/required
		// are present together or not at all; a purely-structural rejection
		// (e.g. close's claimed_by_id) carries only `missing` as before.
		// Reused identically by claim (Tool 3 / bead unblock-tv8.72).
		if v, ok := e.Meta["status"].(string); ok && v != "" {
			details["status"] = v
		}
		if v, ok := e.Meta["required"].(string); ok && v != "" {
			details["required"] = v
		}
		// SPEC §6.2 Tool 13 line 1645-1646 + bead unblock-tv8.21 AC: surface
		// the canonical invariant name as `data.invariant` (kebab-case) for
		// machine-readability. The legacy `rejection_reason` mirror is
		// preserved so existing clients (and the per-call audit-row
		// RejectionReason population above) see the same value at both
		// keys — semantically equivalent in P01, but `invariant` is the
		// machine-targeted field per the spec/bead contract.
		if v, ok := e.Meta["invariant"].(string); ok && v != "" {
			details["invariant"] = v
			details["rejection_reason"] = v
		}
		if v, ok := e.Meta["rejection_reason"].(string); ok && v != "" {
			details["rejection_reason"] = v
		}
		return envelopeError{kind: envelopeKindPreconditionNotMet, message: msg, details: details}

	default:
		return envelopeError{
			kind:    envelopeKindInternal,
			message: msg,
			details: map[string]any{},
		}
	}
}

// mustJSONRaw marshals v into a json.RawMessage; on the impossible
// marshal-failure branch the function emits a fallback envelope so
// the caller never sees a nil Data and the JSON-RPC envelope still
// parses on the wire.
func mustJSONRaw(v any) json.RawMessage {
	b, err := json.Marshal(v)
	if err != nil {
		return json.RawMessage(fmt.Sprintf(
			`{"kind":%q,"message":%q}`,
			envelopeKindInternal,
			"envelope marshal failure: "+err.Error(),
		))
	}
	return b
}
