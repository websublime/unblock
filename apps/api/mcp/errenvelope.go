// errenvelope.go owns the §7 JSON-RPC 2.0 error envelope writer used
// by MCPHandler for failures that happen BEFORE the Go MCP SDK takes
// over the request (auth failures most importantly). Once the SDK is
// handling the request, the SDK's own error response shape is the one
// clients see for tool dispatch failures; this writer only fires on
// the transport-edge auth + parse failures.
//
// SPEC anchor: §7 (lines 1981-2018).
//
//	{
//	  "jsonrpc": "2.0",
//	  "id": <echo>,
//	  "error": {
//	    "code": -32000,
//	    "message": "<one-line human-readable>",
//	    "data": {
//	      "kind": "<MACHINE_CODE>",
//	      "tool": "<tool name or empty>",
//	      "trace_id": "<ULID or empty>",
//	      "details": { /* kind-specific */ }
//	    }
//	  }
//	}
//
// Response semantics:
//
//   - Status code is ALWAYS 200 OK. JSON-RPC 2.0 carries the error
//     state inside the JSON body; HTTP status codes are reserved for
//     transport-level failures (which the SDK or net/http handle).
//     The §7 spec table maps every `kind` to HTTP 200 implicitly.
//   - Content-Type is application/json (NOT text/event-stream). A
//     JSON-RPC error envelope is a single-message response; clients
//     parse it as a normal JSON body. Setting text/event-stream here
//     would force the client to read an SSE frame for what is a
//     single payload, breaking the §5.1 transport contract for
//     pre-SDK auth failures.
//   - id is the JSON-RPC request id from the inbound body when we
//     can parse it; otherwise null. The pre-auth path runs before
//     the SDK touches the body, and reading + replaying req.Body is
//     a cost we pay only on the rare failure path. RFC 8259 §6:
//     null is the canonical JSON null value; the JSON-RPC 2.0 spec
//     §5 line 5: "If there was an error in detecting the id in the
//     Request object … it MUST be Null."
//
// trace_id and tool are pulled from ctx via tracectx.From — the
// MCPHandler entry binds trace_id before this writer is invoked, and
// the tool field stays empty on auth-failure paths because no
// JSON-RPC method is parsed at that point.

package mcp

import (
	"encoding/json"
	"net/http"

	"encore.app/shared/tracectx"
	"encore.dev/rlog"
)

// Error envelope kind constants — the machine-code values returned
// in the §7 envelope's data.kind field. The literals match SPEC §7's
// locked table; do NOT rename without amending the spec.
const (
	envelopeKindUnauthenticated    = "UNAUTHENTICATED"
	envelopeKindForbidden          = "FORBIDDEN"
	envelopeKindNotFound           = "NOT_FOUND"
	envelopeKindValidation         = "VALIDATION"
	envelopeKindAlreadyClaimed     = "ALREADY_CLAIMED"
	envelopeKindCycleDetected      = "CYCLE_DETECTED"
	envelopeKindPreconditionNotMet = "PRECONDITION_NOT_MET"
	envelopeKindConflict           = "CONFLICT"
	envelopeKindInternal           = "INTERNAL"
)

// jsonRPCErrorCode is the JSON-RPC 2.0 error.code we emit for every
// tool-level error. SPEC §7 line 1965: "code: -32000 // JSON-RPC
// reserved range; we always use -32000 for 'tool error'". The full
// classification lives in data.kind — the integer code is the
// JSON-RPC framing requirement, not the spec's error taxonomy.
const jsonRPCErrorCode = -32000

// errEnvelope mirrors the SPEC §7 JSON-RPC error response shape.
// The id field is encoded as json.RawMessage so we can faithfully
// echo whatever JSON value the client sent (string, number, null)
// without coercing it to a Go type.
type errEnvelope struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      json.RawMessage `json:"id"`
	Error   errEnvelopeBody `json:"error"`
}

// errEnvelopeBody is the inner error object per JSON-RPC 2.0 + §7.
type errEnvelopeBody struct {
	Code    int             `json:"code"`
	Message string          `json:"message"`
	Data    errEnvelopeData `json:"data"`
}

// errEnvelopeData is the spec-locked details payload. SPEC §7.
type errEnvelopeData struct {
	Kind    string         `json:"kind"`
	Tool    string         `json:"tool"`
	TraceID string         `json:"trace_id"`
	Details map[string]any `json:"details"`
}

// writeErrorEnvelope serialises an errEnvelope and writes it to w.
// kind is one of the envelopeKind* constants; message is a human
// one-liner; details is the kind-specific data map (use nil for
// `{}`); jsonRPCID is the verbatim JSON-RPC id from the inbound
// request body (or `null` raw when unavailable).
//
// Caller-side contract:
//
//   - MUST be invoked on the response path BEFORE the SDK takes
//     over — the SDK writes its own envelope for in-protocol errors.
//   - MUST be invoked at most once per request; subsequent writes
//     after the body flush are no-ops or trigger an http: superfluous
//     WriteHeader error.
//
// Failure mode: if json.Marshal fails (impossible for the shape we
// emit) the writer falls back to a hard-coded plaintext error and
// logs the marshal failure. The Bearer-only failure path callers
// (MCPHandler) pass values that always marshal cleanly so this
// branch is defensive.
func writeErrorEnvelope(
	w http.ResponseWriter,
	r *http.Request,
	jsonRPCID json.RawMessage,
	kind string,
	message string,
	details map[string]any,
) {
	if details == nil {
		details = map[string]any{}
	}
	if len(jsonRPCID) == 0 {
		jsonRPCID = json.RawMessage("null")
	}

	tool := ""
	traceID := ""
	if r != nil {
		if f, ok := tracectx.From(r.Context()); ok {
			tool = f.Tool
			traceID = f.TraceID
		}
	}

	env := errEnvelope{
		JSONRPC: "2.0",
		ID:      jsonRPCID,
		Error: errEnvelopeBody{
			Code:    jsonRPCErrorCode,
			Message: message,
			Data: errEnvelopeData{
				Kind:    kind,
				Tool:    tool,
				TraceID: traceID,
				Details: details,
			},
		},
	}

	body, err := json.Marshal(env)
	if err != nil {
		// Defensive — the struct shape above is JSON-clean by
		// construction; reaching this branch implies a programmer
		// error in details map values (an unmarshalable type).
		rlog.Error("mcp: writeErrorEnvelope: marshal failed",
			"err", err,
			"kind", kind,
			"trace_id", traceID,
		)
		http.Error(w, "internal error", http.StatusInternalServerError)
		return
	}

	// JSON-RPC error envelopes always travel over HTTP 200; the
	// error state lives inside the body (SPEC §7 file-level doc).
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	if _, writeErr := w.Write(body); writeErr != nil {
		// Client disconnected mid-write — log + return. Cannot
		// surface the failure further; w is already partially
		// written.
		rlog.Debug("mcp: writeErrorEnvelope: write failed",
			"err", writeErr,
			"kind", kind,
			"trace_id", traceID,
		)
	}
}

// parseJSONRPCID attempts to extract the JSON-RPC `id` field from
// body. Returns the raw JSON value as-is (string, number, or null)
// so writeErrorEnvelope can echo it back faithfully. On parse
// failure or absence the function returns json.RawMessage("null")
// per JSON-RPC 2.0 §5 line 5: "If there was an error in detecting
// the id in the Request object … it MUST be Null."
//
// We deliberately do NOT validate the JSON-RPC envelope shape here
// — that is the SDK's job. We just lift the id field so the §7
// error response can echo it. body may be nil or empty (auth
// failures often fire before the body is read); those cases return
// the literal null.
func parseJSONRPCID(body []byte) json.RawMessage {
	if len(body) == 0 {
		return json.RawMessage("null")
	}
	// Decode into a partial shape — only the id field. Other fields
	// are ignored; an unknown shape (e.g. plain text body) hits the
	// json.Unmarshal error path and we return null.
	var probe struct {
		ID json.RawMessage `json:"id"`
	}
	if err := json.Unmarshal(body, &probe); err != nil {
		return json.RawMessage("null")
	}
	if len(probe.ID) == 0 {
		return json.RawMessage("null")
	}
	return probe.ID
}
