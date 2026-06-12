// validate.go is the SHARED argument-validation boundary layer for the
// MCP tool surface (SPEC §7.3 / §7.3.1 / §7.3.2 + §6.2.0a, bead
// unblock-tv8.82).
//
// # Why this layer exists
//
// The live `tools/list` schema is the schema the go-sdk (v1.6.0) would
// otherwise REFLECT from each handler's Go input struct via
// jsonschema.ForType — which carries only `type`, `required` (absence
// of `,omitempty`), and `additionalProperties:false`. It NEVER carries
// `enum` or `minimum`/`maximum`, so the per-tool bounds/enums quoted in
// the §6.2 argument comments and in catalogue.json were never on the
// wire (§7.3.2). Worse, the SDK validates that reflected schema
// PRE-handler (server.go applySchema → resolved.Validate); a PRE-handler
// failure returns a bare `isError` text frame with NO §7 envelope (no
// kind=VALIDATION, no trace_id, no data.field). That is the B3 surface.
//
// # The mechanism (§7.3.2, option (i) — fully uniform)
//
// We OWN every argument-shape dimension (required / enum / type / range /
// additionalProperties) in this shared layer instead of letting the SDK
// own any of them:
//
//   - Tools are registered via the NON-generic sdkServer.AddTool (see
//     registerValidatedTool), which stores the Tool verbatim and runs
//     ZERO applySchema pre-validation. The advertised InputSchema is the
//     FULL rich schema from catalogue.gen.go (enum + minimum/maximum +
//     required + additionalProperties) — so `tools/list` advertises the
//     complete contract for agent discovery (§6.2.0a, NET-NEW).
//   - validateArgs runs at the TOP of every wrapped handler against that
//     same rich schema. On any violation it mints the §7 VALIDATION
//     envelope via the existing errmap.go mapError path (errs.InvalidArgument
//     + Meta{field, reason[, bound]}). No argument violation can surface
//     as a bare isError frame.
//
// The rich schema is read straight from ToolByName(tool).InputSchema
// (the catalogue.gen.go embedded JSON), so the validated contract and the
// advertised contract are the SAME bytes by construction — they cannot
// drift (asserted by TestRegisteredInputSchemaMatchesCatalogue).
//
// # Bounds are ENFORCED — out-of-range REJECTS (§7.3.1, behavior change)
//
// A supplied paginated `limit`/`ready_limit` below the advertised minimum
// (including 0 / negative) OR above the maximum is REJECTED with
// VALIDATION (data.field = the argument, data.bound = the range). The
// server does NOT clamp-to-max or coerce-to-default an out-of-range value.
// An OMITTED limit still takes the per-tool default — the bound check
// applies only to a value the caller actually supplies (absence is not a
// zero). The handlers apply the omitted-default AFTER this layer passes.
//
// SPEC: docs/specs/01-spec-backend-mvp.md §7.3 + §7.3.1 + §7.3.2 +
// §6.2.0a + §7 (VALIDATION envelope) + §10.3 (catalogue).

package mcp

import (
	"context"
	"encoding/json"
	"fmt"
	"math"
	"sort"
	"sync"

	"encore.dev/beta/errs"
	"github.com/google/jsonschema-go/jsonschema"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// richSchemaCache memoises the parsed rich input schema per tool name so
// each tools/call does not re-parse the catalogue JSON. Populated lazily
// by toolInputSchema under richSchemaOnce-per-key guard.
var (
	richSchemaCache   = map[string]*jsonschema.Schema{}
	richSchemaCacheMu sync.RWMutex
)

// toolInputSchema returns the parsed rich JSON Schema for a tool from the
// catalogue.gen.go embedded bytes (the single source of truth that
// tools/list also advertises). It panics if the tool is unknown or the
// embedded schema fails to parse — both are build-time invariants
// (catalogue.gen.go is generated + drift-guarded), so a failure here is a
// programmer error surfaced at boot, never a request-time condition.
func toolInputSchema(tool string) *jsonschema.Schema {
	richSchemaCacheMu.RLock()
	if s, ok := richSchemaCache[tool]; ok {
		richSchemaCacheMu.RUnlock()
		return s
	}
	richSchemaCacheMu.RUnlock()

	ct, ok := ToolByName(tool)
	if !ok {
		panic(fmt.Sprintf("mcp: validate: unknown tool %q (not in catalogue.gen.go)", tool))
	}
	var schema jsonschema.Schema
	if err := json.Unmarshal(ct.InputSchema, &schema); err != nil {
		panic(fmt.Sprintf("mcp: validate: tool %q input_schema does not parse: %v", tool, err))
	}

	richSchemaCacheMu.Lock()
	richSchemaCache[tool] = &schema
	richSchemaCacheMu.Unlock()
	return &schema
}

// validateArgs is the shared §7.3 argument-validation pass. It validates
// the raw JSON tool arguments against the tool's rich input schema and,
// on the FIRST violation it finds, returns a §7 VALIDATION envelope error
// (built via mapError) suitable for returning directly from the handler.
// It returns nil when every argument is in-contract.
//
// Dimensions enforced, in order, against the top-level object schema:
//
//  1. arguments parse as a JSON object (a non-object body → field
//     "arguments").
//  2. additionalProperties:false → any key not in properties → that key
//     is the offending field.
//  3. required[] → a missing required key → that key is the field.
//  4. per-property type → wrong JSON type → that key is the field.
//  5. per-property enum → value outside the closed set → that key is the
//     field, reason lists the allowed values.
//  6. per-property minimum/maximum → an out-of-range number → that key is
//     the field, data.bound = "min..max" (§7.3.1).
//
// state may be nil (unit-test paths); mapError tolerates a nil state.
//
// Determinism: keys are visited in sorted order so the FIRST surfaced
// violation is stable across runs (Go map iteration is randomised). The
// spec is silent on multi-violation ordering; a stable order keeps tests
// and agent retries predictable.
func validateArgs(state *requestState, tool string, rawArgs json.RawMessage) error {
	schema := toolInputSchema(tool)

	// Decode into a generic map so we can inspect presence + JSON type of
	// every supplied argument without a typed struct (which would silently
	// drop unknown keys and coerce types). An empty/absent body is the
	// "no arguments supplied" case — valid iff there are no required keys.
	args := map[string]json.RawMessage{}
	if len(rawArgs) > 0 {
		if err := json.Unmarshal(rawArgs, &args); err != nil {
			return mapError(state, tool, validationErr("arguments", "arguments must be a JSON object", ""))
		}
	}

	// (2) additionalProperties:false — reject unknown keys. The catalogue
	// schema sets additionalProperties:false on every tool except `update`
	// (which intentionally allows extras and owns its own state-dimension
	// sniff in handler_update.go); for `update` schema.AdditionalProperties
	// is a non-nil empty schema (allow-all) so this branch is skipped.
	if additionalPropertiesForbidden(schema) {
		for _, key := range sortedKeys(args) {
			if _, declared := schema.Properties[key]; !declared {
				return mapError(state, tool, validationErr(
					key,
					fmt.Sprintf("unknown argument %q", key),
					"",
				))
			}
		}
	}

	// (3) required[] — a declared-required key that is absent (or present
	// as JSON null) is a missing-required violation.
	for _, req := range schema.Required {
		tok, present := args[req]
		if !present || string(tok) == "null" {
			return mapError(state, tool, validationErr(
				req,
				fmt.Sprintf("missing required argument %q", req),
				"",
			))
		}
	}

	// (4)(5)(6) per-property type / enum / range. Visit supplied keys in
	// sorted order for deterministic first-violation surfacing.
	for _, key := range sortedKeys(args) {
		prop, declared := schema.Properties[key]
		if !declared {
			// Unknown key on an additionalProperties:true tool (update):
			// no per-property constraints to check.
			continue
		}
		tok := args[key]
		if string(tok) == "null" {
			// Explicit JSON null: treated as "not supplied" for an
			// optional field; required-null was already rejected above.
			continue
		}
		if err := validateProperty(state, tool, key, prop, tok); err != nil {
			return err
		}
	}

	return nil
}

// validateProperty checks a single supplied argument value against its
// property schema (type, then enum, then numeric range). Returns a §7
// VALIDATION envelope error on the first violation, nil otherwise.
func validateProperty(state *requestState, tool, field string, prop *jsonschema.Schema, tok json.RawMessage) error {
	// (4) type. We only enforce the scalar/array/object JSON types the
	// P01 schemas use (string, integer, number, boolean, array, object).
	// A property declaring multiple Types (e.g. ["string","null"]) passes
	// if the value matches ANY of them.
	wantTypes := propertyTypes(prop)
	if len(wantTypes) > 0 {
		got := jsonTypeOf(tok)
		if !typeMatches(got, wantTypes) {
			return mapError(state, tool, validationErr(
				field,
				fmt.Sprintf("expected %s, got %s", joinTypes(wantTypes), got),
				"",
			))
		}
	}

	// (5) enum — closed value set. The catalogue enums are all string
	// enums in P01; compare the decoded string against the allowed set.
	if len(prop.Enum) > 0 {
		var got string
		if err := json.Unmarshal(tok, &got); err == nil {
			if !enumContains(prop.Enum, got) {
				return mapError(state, tool, validationErr(
					field,
					fmt.Sprintf("must be one of %s", enumValues(prop.Enum)),
					"",
				))
			}
		}
		// If the value is not a string at all, the type check above
		// already rejected it (enum properties declare type:string).
	}

	// (6) numeric range — minimum/maximum (inclusive). Applies only when
	// the value is a JSON number; non-number values were rejected by the
	// type check. data.bound carries the advertised "min..max" range so
	// the agent can self-correct (§7.3.1).
	if prop.Minimum != nil || prop.Maximum != nil {
		var n float64
		if err := json.Unmarshal(tok, &n); err == nil {
			if prop.Minimum != nil && n < *prop.Minimum {
				return mapError(state, tool, validationErr(
					field, "out of range", boundString(prop),
				))
			}
			if prop.Maximum != nil && n > *prop.Maximum {
				return mapError(state, tool, validationErr(
					field, "out of range", boundString(prop),
				))
			}
		}
	}

	return nil
}

// validationErr builds the canonical errs.InvalidArgument carrying the
// §7 VALIDATION Meta. field always set; reason set when non-empty; bound
// set when non-empty (range violations only — surfaces as data.bound via
// errmap.go). mapError maps InvalidArgument → VALIDATION with data.field
// (and data.reason / data.bound).
func validationErr(field, reason, bound string) error {
	meta := errs.Metadata{"field": field}
	if reason != "" {
		meta["reason"] = reason
	}
	if bound != "" {
		meta["bound"] = bound
	}
	msg := reason
	if msg == "" {
		msg = "invalid argument"
	}
	return &errs.Error{
		Code:    errs.InvalidArgument,
		Message: msg,
		Meta:    meta,
	}
}

// additionalPropertiesForbidden reports whether the schema forbids
// additional properties (additionalProperties:false). The jsonschema-go
// representation of `false` is a non-nil schema whose `Not` is the empty
// schema (the "never" schema); the representation of allow-all (the
// `update` case) is a non-nil EMPTY schema. We distinguish the two by
// remarshalling the additionalProperties value: `false` marshals to the
// literal `false`, allow-all marshals to `{}`.
func additionalPropertiesForbidden(schema *jsonschema.Schema) bool {
	ap := schema.AdditionalProperties
	if ap == nil {
		// Absent → JSON Schema default is "allow additional properties".
		return false
	}
	b, err := json.Marshal(ap)
	if err != nil {
		return false
	}
	return string(b) == "false"
}

// propertyTypes returns the JSON type(s) a property accepts. The
// jsonschema.Schema custom (un)marshaller maps the `type` keyword into
// Type (single) or Types (multiple).
func propertyTypes(prop *jsonschema.Schema) []string {
	if prop.Type != "" {
		return []string{prop.Type}
	}
	return prop.Types
}

// jsonTypeOf reports the JSON type of a raw token using the JSON Schema
// type vocabulary. "integer" is reported for numbers with no fractional
// part so an integer-typed property accepts 5 but rejects 5.5; callers
// treat "integer" as a subtype of "number" via typeMatches.
func jsonTypeOf(tok json.RawMessage) string {
	var v any
	if err := json.Unmarshal(tok, &v); err != nil {
		return "unknown"
	}
	switch t := v.(type) {
	case nil:
		return "null"
	case bool:
		return "boolean"
	case float64:
		if t == math.Trunc(t) && !math.IsInf(t, 0) {
			return "integer"
		}
		return "number"
	case string:
		return "string"
	case []any:
		return "array"
	case map[string]any:
		return "object"
	default:
		return "unknown"
	}
}

// typeMatches reports whether the observed JSON type satisfies any of the
// schema's accepted types. "integer" satisfies a "number" requirement (an
// integer is a number); "integer" also satisfies "integer". The reverse
// is enforced by jsonTypeOf, which only reports "integer" for whole
// numbers, so a fractional value never satisfies an "integer" requirement.
func typeMatches(got string, want []string) bool {
	for _, w := range want {
		if got == w {
			return true
		}
		if w == "number" && got == "integer" {
			return true
		}
	}
	return false
}

// enumContains reports whether v is a member of the (string) enum set.
func enumContains(enum []any, v string) bool {
	for _, e := range enum {
		if s, ok := e.(string); ok && s == v {
			return true
		}
	}
	return false
}

// enumValues renders the allowed enum members as a quoted comma list for
// the VALIDATION reason text (e.g. `"P0", "P1", "P2"`).
func enumValues(enum []any) string {
	parts := make([]string, 0, len(enum))
	for _, e := range enum {
		if s, ok := e.(string); ok {
			parts = append(parts, fmt.Sprintf("%q", s))
		}
	}
	return joinComma(parts)
}

// boundString renders the advertised inclusive range as "min..max" for
// data.bound. Both bounds are present on every paginated limit property
// in P01; a half-open bound degrades gracefully to "min.." / "..max".
func boundString(prop *jsonschema.Schema) string {
	lo, hi := "", ""
	if prop.Minimum != nil {
		lo = trimFloat(*prop.Minimum)
	}
	if prop.Maximum != nil {
		hi = trimFloat(*prop.Maximum)
	}
	return lo + ".." + hi
}

// trimFloat renders a float that holds an integer value without a
// trailing ".0" (so 50.0 → "50", matching the §6.2.0a "1..50" prose).
func trimFloat(f float64) string {
	if f == math.Trunc(f) && !math.IsInf(f, 0) {
		return fmt.Sprintf("%d", int64(f))
	}
	return fmt.Sprintf("%g", f)
}

func joinTypes(types []string) string { return joinComma(types) }

func joinComma(parts []string) string {
	out := ""
	for i, p := range parts {
		if i > 0 {
			out += ", "
		}
		out += p
	}
	return out
}

// sortedKeys returns the map keys in lexical order for deterministic
// first-violation surfacing.
func sortedKeys(m map[string]json.RawMessage) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

// registerValidatedTool registers a typed tool handler against sdkServer
// such that:
//
//   - the advertised `tools/list` InputSchema is the FULL rich schema
//     from catalogue.gen.go (enum/min/max/required/additionalProperties),
//     for agent discovery (§6.2.0a), and
//   - the go-sdk runs NO pre-handler applySchema validation (the
//     non-generic Server.AddTool path stores the Tool verbatim and never
//     resolves an input schema), so no argument violation can surface as
//     a bare isError frame, and
//   - validateArgs (§7.3) runs at the boundary, before the typed handler,
//     minting §7 VALIDATION on any argument-shape violation.
//
// This is the generalisation of the handler_update.go precedent into a
// single shared boundary layer (§7.3.2). The helper replicates the
// go-sdk generic-handler output path faithfully (StructuredContent +
// TextContent mirror + output-schema validation) so existing
// structured-echoes-text assertions stay green.
//
// in is a fresh zero In on every call (the closure constructs its own),
// so the helper is safe for concurrent dispatch.
func registerValidatedTool[In, Out any](
	s *sdkmcp.Server,
	name, description string,
	outputSchema *jsonschema.Schema,
	typed func(context.Context, *sdkmcp.CallToolRequest, In) (*sdkmcp.CallToolResult, Out, error),
) {
	richInput := toolInputSchema(name)

	// Derive (or accept) the output schema the same way the generic SDK
	// path would, so the advertised outputSchema and the output validation
	// are unchanged from the reflected default. A caller may pass an
	// explicit schema (handler_milestone_tree.go does) to override the
	// reflected shape.
	outSchema := outputSchema
	if outSchema == nil {
		derived, err := jsonschema.For[Out](nil)
		if err != nil {
			panic(fmt.Sprintf("mcp: validate: output schema inference for %q: %v", name, err))
		}
		outSchema = derived
	}
	outResolved, err := outSchema.Resolve(&jsonschema.ResolveOptions{ValidateDefaults: true})
	if err != nil {
		panic(fmt.Sprintf("mcp: validate: output schema resolve for %q: %v", name, err))
	}

	handler := func(ctx context.Context, req *sdkmcp.CallToolRequest) (*sdkmcp.CallToolResult, error) {
		// §7.3 uniform argument validation BEFORE the typed handler runs.
		// bindTool registers the tool name on the audit row exactly as
		// the typed handlers do; on a validation reject mapError (called
		// inside validateArgs) flips the audit row to result_kind=error.
		state := bindTool(req, name)
		if err := validateArgs(state, name, req.Params.Arguments); err != nil {
			return nil, err
		}

		// Unmarshal the validated arguments into the typed input. We use
		// stdlib json.Unmarshal (unknown keys already rejected by
		// validateArgs for additionalProperties:false tools; tolerated
		// for `update`, which owns its own raw sniff). A malformed body
		// was already rejected by validateArgs.
		var in In
		if len(req.Params.Arguments) > 0 {
			if err := json.Unmarshal(req.Params.Arguments, &in); err != nil {
				return nil, mapError(state, name, validationErr("arguments", "arguments must be a JSON object", ""))
			}
		}

		res, out, herr := typed(ctx, req, in)
		if herr != nil {
			// A handler that already returns a *jsonrpc.Error (the §7
			// path via mapError) is forwarded verbatim; any other error
			// is wrapped by the SDK as an isError tool result. The typed
			// handlers always return mapError output, so this is the §7
			// path in practice.
			return nil, herr
		}

		return marshalToolOutput(res, out, outResolved)
	}

	// Non-generic registration: stores richInput verbatim as the
	// advertised InputSchema and runs our handler directly. No applySchema.
	s.AddTool(&sdkmcp.Tool{
		Name:         name,
		Description:  description,
		InputSchema:  richInput,
		OutputSchema: outSchema,
	}, handler)
}

// marshalToolOutput replicates the go-sdk generic-handler output path
// (server.go toolForErr): marshal Out → StructuredContent, validate
// against the resolved output schema, and mirror into a TextContent
// block when the handler did not set Content itself. Keeps the wire
// shape byte-identical to the previous generic AddTool registration so
// assertStructuredEchoesText and friends stay green.
func marshalToolOutput[Out any](res *sdkmcp.CallToolResult, out Out, outResolved *jsonschema.Resolved) (*sdkmcp.CallToolResult, error) {
	if res == nil {
		res = &sdkmcp.CallToolResult{}
	}

	outBytes, err := json.Marshal(out)
	if err != nil {
		return nil, fmt.Errorf("marshaling output: %w", err)
	}
	outJSON := json.RawMessage(outBytes)
	if outResolved != nil {
		v := map[string]any{}
		if len(outJSON) > 0 {
			if err := json.Unmarshal(outJSON, &v); err != nil {
				return nil, fmt.Errorf("unmarshaling output for validation: %w", err)
			}
		}
		if err := outResolved.ApplyDefaults(&v); err != nil {
			return nil, fmt.Errorf("applying output defaults: %w", err)
		}
		if err := outResolved.Validate(&v); err != nil {
			return nil, fmt.Errorf("validating tool output: %w", err)
		}
		if outBytes, err = json.Marshal(v); err != nil {
			return nil, fmt.Errorf("re-marshaling output: %w", err)
		}
		outJSON = json.RawMessage(outBytes)
	}

	res.StructuredContent = outJSON
	if res.Content == nil {
		res.Content = []sdkmcp.Content{&sdkmcp.TextContent{Text: string(outJSON)}}
	}
	return res, nil
}
