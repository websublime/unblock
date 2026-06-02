// warnings.go owns the §7.1 success-side warnings contract — the
// typed, optional `warnings` array a tool MAY carry INSIDE its
// `structuredContent` success result when the primary mutation
// succeeded but left a non-fatal residue the caller deserves to
// observe (e.g. a dropped `intent_comment` on `set_state`).
//
// # Why structuredContent, not _meta or a top-level sibling
//
// SPEC §7.1 pins the home of warnings to `structuredContent` and
// rejects the two alternatives for concrete go-sdk v1.6.0 reasons:
//
//   - A top-level `CallToolResult` sibling is unreachable: the
//     go-sdk v1.6.0 CallToolResult struct exposes exactly Meta
//     (`_meta`), Content, StructuredContent and IsError, with no
//     index-signature passthrough and no custom MarshalJSON — an
//     undeclared top-level field simply does not serialise.
//   - `_meta` is a map[string]any whose keys are string literals,
//     invisible to the NFR-10 snake_case gate
//     (`grep -rnE 'json:"[A-Z]' apps/api/`). Routing through a
//     TYPED struct field is deliberate so the gate actually
//     inspects the wire keys.
//
// jsonschema-go infers `additionalProperties: false` on a tool's
// inferred output schema and the SDK validates structuredContent
// against it; an undeclared key would fail validation. The warning
// channel MUST therefore be a declared field of the tool's typed Out
// struct. Embedding the shared WithWarnings struct promotes
// `warnings` into the schema as a sibling of `item`, preserving the
// single-object shape and additionalProperties:false.
//
// # Shape (pinned — shared embedded struct, one wired producer)
//
// SPEC §7.1 weighs (A) re-declaring a per-tool Warnings field on each
// Out struct (rejected: duplicates the Warning definition N times and
// invites json-tag / omitempty drift) against (B, PINNED) a single
// shared WithWarnings struct embedded into the Out structs that can
// emit warnings. Only setStateOut embeds it in P01/P02 — the one
// wired producer (code `intent_comment_dropped`). Future producers
// (e.g. a P02 `cascade_delayed`) reuse these types with zero
// re-definition: add a registry row (§7.1) plus an Out-struct
// producer; no result-shape change is required.
//
// SPEC: docs/specs/01-spec-backend-mvp.md § 7.1 (success-side
// warnings) + § 6.2 Tool 13 (intent_comment partial-failure) +
// § 8.1.1 (warning_codes audit column) + § 3.6 (snake_case wire).

package mcp

// warningCodeIntentCommentDropped is the only §7.1 warning code wired
// in P01/P02. Emitted by Tool 13 (`set_state`) when the state
// mutation committed but the best-effort `intent_comment`
// AppendComment failed (DECISION 2026-05-18, unblock-tv8.21). It is
// both the wire `code` value (§7.1 registry) and the audited
// `warning_codes` entry (§8.1.1). Keep the literal in sync with the
// §7.1 registry table.
const warningCodeIntentCommentDropped = "intent_comment_dropped"

// Warning is the canonical §7.1 warning object carried on a tool's
// SUCCESS result inside structuredContent. Every key is snake_case
// per §3.6 (the typed struct tags are inspected by the NFR-10 gate).
//
//   - Code is a machine-stable identifier from the §7.1 registry.
//   - Message is a one-line human-readable summary.
//   - Details is optional (omitempty) per-code structured context;
//     its shape is defined per code in the §7.1 registry and its
//     keys are snake_case. Details MUST NOT carry large or sensitive
//     payloads (e.g. comment bodies) — for intent_comment_dropped it
//     echoes kind/status only; the body length + sha256 go to rlog
//     diagnostics, never the wire.
type Warning struct {
	Code    string         `json:"code"`
	Message string         `json:"message"`
	Details map[string]any `json:"details,omitempty"`
}

// WithWarnings is the shared embeddable carrier for the §7.1
// success-side warnings array. Out structs that can emit warnings
// embed it (currently only setStateOut); jsonschema-go promotes the
// embedded Warnings field into the tool's output schema as a sibling
// of the tool's primary payload. The `omitempty` tag means a result
// with no warnings omits the key entirely — the no-warning path emits
// exactly the pre-existing shape, so the field is purely additive.
type WithWarnings struct {
	Warnings []Warning `json:"warnings,omitempty"`
}
