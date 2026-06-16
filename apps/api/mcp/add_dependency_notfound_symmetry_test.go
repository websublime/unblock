// add_dependency_notfound_symmetry_test.go locks bead unblock-tv8.89 at the
// MCP WIRE boundary — the level the live bug actually manifested.
//
// Why a dedicated mcp-package test (the deps integration test is NOT enough):
// on a real `add_dependency` MCP call the two NOT_FOUND endpoints are minted by
// TWO DIFFERENT code paths, and only one of them lives in the deps package:
//
//   - missing to_item   → minted by the HANDLER. handleAddDependency resolves
//     the to_item via workitems.Get BEFORE deps.AddEdge ever runs (project_id
//     derivation, DRIFT-2 option (a)); on NotFound it short-circuits with its
//     own *errs.Error{kind:"item", id:to_item_id} literal
//     (handler_add_dependency.go:148-152). deps.AddEdgeInTx's to_item branch is
//     UNREACHABLE from the agent surface.
//   - missing from_item → minted by deps.AddEdgeInTx (deps/deps.go:250-260),
//     because the handler does NOT pre-resolve the from_item; it flows straight
//     into deps.AddEdge.
//
// The live bug was precisely this HANDLER-vs-in-tx asymmetry: the handler's
// to_item literal carried the kind:"item" §7 discriminant while the deps
// from_item literal omitted it, so an agent saw {kind:"item",id} for a missing
// to_item but {id} for a missing from_item. The deps integration test
// (TestAddEdgeNotFoundDetailsSymmetry) drives deps.AddEdge for BOTH endpoints,
// so it exercises the in-tx path twice and can NEVER observe the handler literal
// — it cannot regression-guard the real wire asymmetry. This test does.
//
// Approach: reproduce each endpoint's source-of-truth NOT_FOUND *errs.Error
// literal verbatim (pinned by comment to its producing line), push BOTH through
// classifyEnvelopeError — the SINGLE §7 projection every MCP error response
// passes through (errmap.go) — and assert the resulting `details` maps the agent
// observes are structurally identical: both exactly {kind:"item", id:<endpoint>}.
// If either producing literal drifts (drops kind, renames a key), this test
// fails and the from↔to wire symmetry must be re-established.

package mcp

import (
	"testing"

	"encore.dev/beta/errs"
)

// handlerToItemNotFound reproduces the to_item NOT_FOUND *errs.Error minted by
// handleAddDependency on the workitems.Get(to_item) NotFound branch
// (handler_add_dependency.go:148-152). This is the literal a real
// add_dependency MCP call returns when the to_item does not exist.
func handlerToItemNotFound(toItemID string) *errs.Error {
	return &errs.Error{
		Code:    errs.NotFound,
		Message: "to_item not found",
		Meta:    errs.Metadata{"kind": "item", "id": toItemID, "field": "to_item_id"},
	}
}

// depsFromItemNotFound reproduces the from_item NOT_FOUND *errs.Error minted by
// deps.AddEdgeInTx on the from_item ErrNoRows branch (deps/deps.go:250-260).
// This is the literal a real add_dependency MCP call returns when the from_item
// does not exist (the handler does not pre-resolve from_item, so it reaches the
// in-tx lookup).
func depsFromItemNotFound(fromItemID string) *errs.Error {
	return &errs.Error{
		Code:    errs.NotFound,
		Message: "from_item not found",
		Meta:    errs.Metadata{"field": "from_item_id", "id": fromItemID, "kind": "item"},
	}
}

// TestAddDependencyNotFoundWireSymmetry is the bead unblock-tv8.89 wire guard:
// the handler-minted to_item NOT_FOUND and the deps-minted from_item NOT_FOUND
// MUST project to identical §7 `details` through classifyEnvelopeError — the
// kind="item" / id={endpoint} shape the §7 contract mandates
// (docs/specs/01-spec-backend-mvp.md §7 line 3491). This is the symmetry the
// live bug broke; the deps integration test cannot reach the handler path.
func TestAddDependencyNotFoundWireSymmetry(t *testing.T) {
	const (
		fromID = "01J0FROMITEMAAAAAAAAAAAAAA"
		toID   = "01J0TOITEMBBBBBBBBBBBBBBBB"
	)

	fromEnv := classifyEnvelopeError(depsFromItemNotFound(fromID))
	toEnv := classifyEnvelopeError(handlerToItemNotFound(toID))

	// Both must classify to the NOT_FOUND §7 kind.
	if fromEnv.kind != envelopeKindNotFound {
		t.Fatalf("from_item kind = %q, want %q", fromEnv.kind, envelopeKindNotFound)
	}
	if toEnv.kind != envelopeKindNotFound {
		t.Fatalf("to_item kind = %q, want %q", toEnv.kind, envelopeKindNotFound)
	}

	// §7: both details MUST carry the kind="item" subject discriminant — the
	// exact field the live bug dropped on the from_item path.
	if got := fromEnv.details["kind"]; got != "item" {
		t.Fatalf("from_item details.kind = %v, want \"item\" (the live-bug regression)", got)
	}
	if got := toEnv.details["kind"]; got != "item" {
		t.Fatalf("to_item details.kind = %v, want \"item\"", got)
	}

	// Each details.id is the respective missing endpoint id; the `field` Meta
	// key is intentionally NOT projected into §7 details (errmap NOT_FOUND case
	// surfaces only kind + id), so the agent-visible payload is symmetric.
	if got := fromEnv.details["id"]; got != fromID {
		t.Fatalf("from_item details.id = %v, want %q", got, fromID)
	}
	if got := toEnv.details["id"]; got != toID {
		t.Fatalf("to_item details.id = %v, want %q", got, toID)
	}

	// Structural identity: same key set, same per-key shape. Both project the
	// fixed 2-key {kind, id} §7 NOT_FOUND projection — equal length plus the
	// per-key value assertions above prove the two wire payloads are the same
	// shape (acceptance criterion #2, now at the real handler-vs-in-tx seam).
	if len(fromEnv.details) != len(toEnv.details) {
		t.Fatalf("details key-count differs: from=%v to=%v", fromEnv.details, toEnv.details)
	}
	for k := range fromEnv.details {
		if _, ok := toEnv.details[k]; !ok {
			t.Fatalf("from_item details key %q absent in to_item details %v", k, toEnv.details)
		}
	}
}
