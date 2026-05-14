// handler_update.go owns the §6.2 Tool 5 (`update`) handler — the
// editable-column mutator (title, body, priority, milestone_id, labels).
//
// State dimensions (impl_state / review_state / qa_state / pipeline_state)
// are EXPLICITLY rejected here per SPEC §6.2 Tool 5 line 1316 ("Does NOT
// touch state dimensions — use set_state for those"). The bead AC #1
// requires a §7 VALIDATION envelope with `data.field=<offending field>`
// when any of the four forbidden keys appears in the JSON arguments.
//
// Why a raw-JSON sniff and not the SDK's schema validator:
//
//   - github.com/google/jsonschema-go (v0.4.3) auto-infers
//     `additionalProperties: false` for struct types, so a typed In
//     would normally cause the SDK to reject unknown keys at
//     applySchema time. BUT the SDK rejects them by SetError'ing the
//     CallToolResult (server.go:323-326) — the §7 error envelope path
//     never runs and the wire response is a content-text error, not
//     `error.data.kind=VALIDATION`.
//
//   - We override Tool.InputSchema with an explicit object schema that
//     ALLOWS additional properties so the SDK's auto-inference path
//     does not run. Forbidden keys then survive into req.Params.Arguments
//     where the handler can sniff them and emit the spec-compliant
//     §7 VALIDATION envelope via mapError.
//
//   - The typed `in updateIn` struct still receives the legitimate
//     fields (title/body/priority/milestone_id/labels) via the SDK's
//     own unmarshal; unknown keys are silently dropped by
//     internaljson.Unmarshal, which is the desired outcome AFTER the
//     sniff has confirmed none of the four forbidden state-dimension
//     keys are present.
//
// milestone_id wire semantics per SPEC §6.2 Tool 5 line 1314
// (`<ULID|null; optional>`): JSON `null` clears the column, a string
// sets it, absence leaves it unchanged. The handler distinguishes the
// three cases by sniffing the raw JSON before relying on the typed
// unmarshal (Go's zero string cannot represent "absent vs explicit
// null vs empty string" on its own).
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 5 (lines 1306-1320)
// + § 7 (error envelope) + § 4.4 (workitems.Update pointer-field
// contract).

package mcp

import (
	"context"
	"encoding/json"

	"encore.app/workitems"
	"encore.dev/beta/errs"
	"github.com/google/jsonschema-go/jsonschema"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// forbiddenUpdateFields enumerates the four state-dimension column
// names §6.2 Tool 5 prohibits. They are owned by Tool 13 (set_state).
// Iteration order is stable so the rejection error consistently names
// the first-discovered offender (spec is silent on ordering; we pick
// the spec line order for predictability).
var forbiddenUpdateFields = []string{
	"impl_state",
	"review_state",
	"qa_state",
	"pipeline_state",
}

// updateIn is the typed input to handleUpdate. Fields are pointers so
// the handler can faithfully forward "unchanged vs explicit set vs
// explicit clear" semantics to workitems.Update which uses the same
// pointer-nil-is-unchanged convention (workitems/workitems.go § Update).
//
// MilestoneID is omitted from the typed struct because Go's
// encoding/json cannot distinguish `null` from absent on a *string
// field once unmarshalled — both yield nil. The handler reads the
// milestone_id intent directly from the sniffed raw JSON map below
// (presence + null-vs-string) and constructs the
// workitems.UpdateRequest.MilestoneID pointer accordingly.
type updateIn struct {
	ItemID   string    `json:"item_id"`
	Title    *string   `json:"title,omitempty"`
	Body     *string   `json:"body,omitempty"`
	Priority *string   `json:"priority,omitempty"`
	Labels   *[]string `json:"labels,omitempty"`
}

type updateOut struct {
	Item primeItem `json:"item"`
}

// registerHandleUpdate is invoked by transport.go's init — see the
// toolRegistrars rationale there.
//
// Tool.InputSchema is set explicitly to an object schema with
// AdditionalProperties = true so the SDK's auto-inferred
// `additionalProperties: false` does not fire on impl_state/etc. The
// handler enforces the rejection itself with a §7-shaped error.
func registerHandleUpdate(s *sdkmcp.Server) {
	sdkmcp.AddTool(s, &sdkmcp.Tool{
		Name: "update",
		Description: "Update editable item columns (title, body, priority, " +
			"milestone_id, labels). State dimensions (impl_state, " +
			"review_state, qa_state, pipeline_state) are rejected with " +
			"VALIDATION — use set_state instead. SPEC § 6.2 Tool 5.",
		// AdditionalProperties: true here keeps the SDK's
		// applySchema from rejecting unknown keys before the handler
		// runs (which would surface as an SDK IsError content frame,
		// not the §7 envelope the spec mandates). The handler then
		// sniffs req.Params.Arguments for the forbidden state-
		// dimension keys and produces the canonical envelope.
		InputSchema: &jsonschema.Schema{
			Type: "object",
			Properties: map[string]*jsonschema.Schema{
				"item_id":      {Type: "string"},
				"title":        {Type: "string"},
				"body":         {Type: "string"},
				"priority":     {Type: "string"},
				"milestone_id": {Types: []string{"string", "null"}},
				"labels": {
					Type:  "array",
					Items: &jsonschema.Schema{Type: "string"},
				},
			},
			Required:             []string{"item_id"},
			AdditionalProperties: trueSchemaForUpdate(),
		},
	}, handleUpdate)
}

// trueSchemaForUpdate returns the JSON-Schema sentinel for "any
// additional properties are allowed". The jsonschema-go library
// distinguishes "no constraint" (nil) from "explicit allow" via a
// pointer to an empty schema; we use the latter to make the intent
// reviewable and to prevent a future re-inference pass from collapsing
// nil into the default falseSchema().
func trueSchemaForUpdate() *jsonschema.Schema {
	return &jsonschema.Schema{}
}

func handleUpdate(ctx context.Context, req *sdkmcp.CallToolRequest, in updateIn) (*sdkmcp.CallToolResult, updateOut, error) {
	tool := "update"
	state := bindTool(req, tool)

	if _, ok := identityFromReq(req); !ok {
		return nil, updateOut{}, mapError(state, tool, errMissingIdentityErr())
	}

	mcpCtx, err := withIdentityFromReq(ctx, req)
	if err != nil {
		return nil, updateOut{}, mapError(state, tool, err)
	}

	// Sniff the raw JSON arguments BEFORE trusting the typed `in` —
	// internaljson.Unmarshal silently drops unknown keys, so a forbidden
	// state-dimension field would otherwise pass through unnoticed.
	// Decoding into map[string]json.RawMessage lets us inspect both
	// presence (key exists) and the literal token (string vs `null`)
	// for milestone_id without re-parsing every value.
	var raw map[string]json.RawMessage
	if len(req.Params.Arguments) > 0 {
		if err := json.Unmarshal(req.Params.Arguments, &raw); err != nil {
			return nil, updateOut{}, mapError(state, tool, &errs.Error{
				Code:    errs.InvalidArgument,
				Message: "arguments must be a JSON object",
				Meta:    errs.Metadata{"field": "arguments"},
			})
		}
	}

	// Reject any state-dimension field per SPEC §6.2 Tool 5 + bead AC #1.
	// We surface the first offender to give the caller a single, actionable
	// field name; downstream callers retrying with the field stripped will
	// surface the next offender on the subsequent attempt.
	for _, field := range forbiddenUpdateFields {
		if _, present := raw[field]; present {
			return nil, updateOut{}, mapError(state, tool, &errs.Error{
				Code:    errs.InvalidArgument,
				Message: "state dimensions are read-only on update — use set_state",
				Meta: errs.Metadata{
					"field":  field,
					"reason": "state dimensions are not editable via update",
				},
			})
		}
	}

	if in.ItemID == "" {
		return nil, updateOut{}, mapError(state, tool, &errs.Error{
			Code:    errs.InvalidArgument,
			Message: "missing item_id",
			Meta:    errs.Metadata{"field": "item_id"},
		})
	}
	if state != nil && state.Call != nil {
		state.Call.ItemID = in.ItemID
	}

	// milestone_id three-way resolution from the raw JSON:
	//   absent          → pointer stays nil → workitems.Update leaves it unchanged.
	//   explicit null   → pointer to empty string → workitems.Update clears the column.
	//   string value    → pointer to the value → workitems.Update sets the column.
	//
	// The typed `in updateIn` deliberately omits MilestoneID for this
	// reason — see updateIn doc comment.
	var milestonePtr *string
	if tok, present := raw["milestone_id"]; present {
		if string(tok) == "null" {
			empty := ""
			milestonePtr = &empty
		} else {
			var s string
			if err := json.Unmarshal(tok, &s); err != nil {
				return nil, updateOut{}, mapError(state, tool, &errs.Error{
					Code:    errs.InvalidArgument,
					Message: "milestone_id must be a string or null",
					Meta:    errs.Metadata{"field": "milestone_id"},
				})
			}
			milestonePtr = &s
		}
	}

	item, err := workitems.Update(mcpCtx, &workitems.UpdateRequest{
		ItemID:      in.ItemID,
		Title:       in.Title,
		Body:        in.Body,
		Priority:    in.Priority,
		MilestoneID: milestonePtr,
		Labels:      in.Labels,
	})
	if err != nil {
		return nil, updateOut{}, mapError(state, tool, err)
	}

	out := updateOut{Item: itemToPrime(*item)}
	if state != nil && state.Call != nil {
		state.Call.ResultKind = ResultOK
		state.Call.ProjectID = item.ProjectID
	}
	return nil, out, nil
}
