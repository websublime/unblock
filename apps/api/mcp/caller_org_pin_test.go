// caller_org_pin_test.go pins the SPEC §10.1.1 item-(c) contract: every MCP
// write handler ALWAYS sets the backing write RPC's CallerOrgID from the
// Bearer-resolved identity.OrgID, never from the wire input. This makes the
// empty-CallerOrgID no-op branch (the row-level tenant gate's $caller = ''
// escape hatch, reserved for trusted internal non-MCP callers) UNREACHABLE
// from the agent surface — the gate is always live on every tool call.
//
// Why a source-level assertion rather than (only) the empirical foreign-id ⇒
// NOT_FOUND suite in exitcriteriontest/write_surface_cross_tenant_test.go:
// the cross-tenant suite proves each handler pins a value that REJECTS the
// foreign org, and the happy-path suites prove it ADMITS the owning org —
// together bracketing CallerOrgID == identity.OrgID. But that is empirical and
// per-tool; it cannot prove the negative for a handler added LATER. This test
// is the single-point structural guard Ada's §10.1.1 contract item-(c) names:
// it enumerates every write-handler source file and asserts (a) it constructs
// its backing request struct with `CallerOrgID: identity.OrgID` and (b) it
// never assigns CallerOrgID from the wire (`in.`/`req.` input). A future
// handler that forwards the wire value — or forgets the pin — fails here at
// compile/test time, before it can ship a confused-deputy IDOR.

package mcp

import (
	"os"
	"path/filepath"
	"regexp"
	"runtime"
	"testing"
)

// writeHandlerFiles is the canonical set of MCP write-handler source files that
// forward a CallerOrgID tenant gate to a backing write RPC (workitems / deps).
// This list is the §10.1.1 write surface. Read-only handlers (prime, ready,
// show, list, search, get_state, list_label) carry no CallerOrgID channel and
// gate via rbac.For at the read SQL, so they are deliberately excluded.
//
// handler_create.go is also excluded: item creation is scope-PINNED via the
// new row's OrgID (= identity.OrgID), not a by-id write needing the CallerOrgID
// row-level predicate — it is covered by createOrgPinFile below with its own
// assertion.
var writeHandlerFiles = []string{
	"handler_add_dependency.go",
	"handler_assign_item.go",
	"handler_claim.go",
	"handler_close.go",
	"handler_comment.go",
	"handler_create_label.go",
	"handler_create_milestone.go",
	"handler_delete_label.go",
	"handler_milestone_tree.go",
	"handler_promote.go",
	"handler_remove_dependency.go",
	"handler_set_state.go",
	"handler_update.go",
	"handler_update_label.go",
	"handler_update_milestone.go",
}

// createOrgPinFile is the create-item handler, gated via OrgID (scope pin), not
// CallerOrgID (by-id predicate). Asserted separately below.
const createOrgPinFile = "handler_create.go"

// pinPattern matches `CallerOrgID: identity.OrgID` allowing arbitrary internal
// whitespace (gofmt aligns struct fields), e.g. `CallerOrgID:   identity.OrgID`.
var pinPattern = regexp.MustCompile(`CallerOrgID:\s*identity\.OrgID\b`)

// wirePinPattern matches any assignment of CallerOrgID from a wire-derived
// source — the request input (`in.…`) or the inbound request struct (`req.…`).
// Such an assignment would let an agent spoof its tenant and is forbidden.
var wirePinPattern = regexp.MustCompile(`CallerOrgID:\s*(?:in|req)\.`)

// mcpSourceDir resolves the directory holding this test file (the mcp package
// source), mirroring the runtime.Caller anchor used by catalogue_test.go.
func mcpSourceDir(t *testing.T) string {
	t.Helper()
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatalf("runtime.Caller failed; cannot locate mcp package source dir")
	}
	return filepath.Dir(thisFile)
}

// TestWriteHandlers_PinCallerOrgIDFromIdentity is the §10.1.1 item-(c) contract
// guard: every write handler pins CallerOrgID from identity.OrgID and never
// from the wire, so the no-op branch of the row-level tenant gate is
// unreachable from the agent surface.
func TestWriteHandlers_PinCallerOrgIDFromIdentity(t *testing.T) {
	dir := mcpSourceDir(t)

	for _, name := range writeHandlerFiles {
		name := name
		t.Run(name, func(t *testing.T) {
			src, err := os.ReadFile(filepath.Join(dir, name))
			if err != nil {
				t.Fatalf("read %s: %v", name, err)
			}
			body := string(src)

			// (a) The handler MUST pin CallerOrgID from identity.OrgID.
			if !pinPattern.MatchString(body) {
				t.Errorf("%s: missing required pin `CallerOrgID: identity.OrgID` — "+
					"every write handler MUST pin CallerOrgID from the Bearer-resolved "+
					"identity (SPEC §10.1.1 item-c). Without it the row-level tenant "+
					"gate's empty-CallerOrgID no-op is reachable from the agent surface "+
					"(confused-deputy IDOR).", name)
			}

			// (b) The handler MUST NOT source CallerOrgID from the wire input.
			if loc := wirePinPattern.FindString(body); loc != "" {
				t.Errorf("%s: forbidden wire-sourced CallerOrgID assignment (%q) — "+
					"CallerOrgID is an INTERNAL channel pinned only from identity.OrgID; "+
					"accepting it from the wire lets an agent spoof its tenant (SPEC §10.1.1).",
					name, loc)
			}
		})
	}
}

// TestCreateHandler_PinsOrgFromIdentity guards the create-item handler, whose
// tenant scope is set by the new row's OrgID (not a by-id CallerOrgID
// predicate). It must likewise come from identity.OrgID, never the wire.
func TestCreateHandler_PinsOrgFromIdentity(t *testing.T) {
	dir := mcpSourceDir(t)
	src, err := os.ReadFile(filepath.Join(dir, createOrgPinFile))
	if err != nil {
		t.Fatalf("read %s: %v", createOrgPinFile, err)
	}
	body := string(src)

	orgPin := regexp.MustCompile(`OrgID:\s*identity\.OrgID\b`)
	if !orgPin.MatchString(body) {
		t.Errorf("%s: missing required pin `OrgID: identity.OrgID` — the create "+
			"handler MUST scope the new row to the Bearer-resolved identity, never "+
			"the wire (SPEC §10.1.1).", createOrgPinFile)
	}

	wireOrg := regexp.MustCompile(`OrgID:\s*(?:in|req)\.`)
	if loc := wireOrg.FindString(body); loc != "" {
		t.Errorf("%s: forbidden wire-sourced OrgID assignment (%q) — the new item's "+
			"org scope MUST be pinned from identity.OrgID, not accepted from the wire.",
			createOrgPinFile, loc)
	}
}

// TestWriteHandlerSet_IsComplete is a drift canary: if a new handler_*.go file
// forwards a CallerOrgID to a backing RPC but is NOT listed in
// writeHandlerFiles, the §10.1.1 item-(c) guard above would silently skip it.
// This scans every handler source file for a CallerOrgID assignment and asserts
// the producing set exactly equals writeHandlerFiles, forcing the list (and
// thus the pin assertion) to stay exhaustive as the write surface grows.
func TestWriteHandlerSet_IsComplete(t *testing.T) {
	dir := mcpSourceDir(t)

	matches, err := filepath.Glob(filepath.Join(dir, "handler_*.go"))
	if err != nil {
		t.Fatalf("glob handler files: %v", err)
	}

	expected := make(map[string]struct{}, len(writeHandlerFiles))
	for _, n := range writeHandlerFiles {
		expected[n] = struct{}{}
	}

	// Any CallerOrgID: assignment marks a file as part of the write surface.
	callerOrgAssign := regexp.MustCompile(`CallerOrgID:`)

	for _, path := range matches {
		base := filepath.Base(path)
		if base == createOrgPinFile {
			continue // create handler uses OrgID, not CallerOrgID — asserted elsewhere
		}
		src, err := os.ReadFile(path)
		if err != nil {
			t.Fatalf("read %s: %v", base, err)
		}
		hasCaller := callerOrgAssign.Match(src)
		_, listed := expected[base]

		if hasCaller && !listed {
			t.Errorf("%s sets CallerOrgID but is NOT in writeHandlerFiles — add it so "+
				"the §10.1.1 item-c pin assertion covers it (drift canary).", base)
		}
		if !hasCaller && listed {
			t.Errorf("%s is in writeHandlerFiles but sets no CallerOrgID — remove it or "+
				"restore the pin (stale entry / dropped gate).", base)
		}
	}
}
