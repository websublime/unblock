// catalogue_test.go exercises the §10.3 MCP tool catalogue invariants
// pinned by D-7 (unblock-tv8.22):
//
//  - catalogue.json contains exactly 14 P01 tool entries (AC #1)
//  - every tool carries name, description, input_schema, output_schema
//    (AC #1)
//  - transitions[] is an empty array (AC #2)
//  - each tools[].name matches the canonical Name registered against
//    sdkServer in transport.go::toolRegistrars (single source of truth
//    assertion — drift between the catalogue and the live SDK is the
//    canonical D-7 failure mode)
//  - Catalogue, ToolNames, ToolByName generated helpers agree with the
//    on-disk JSON
//
// Per the orchestrator DECISION on this bead (2026-05-21), this is the
// minimum P01 test. The cross-check against the SDK's actual tools/list
// wire response is captured by the recordtoolcall_test.go / d1_transport_test.go
// fixtures and is the canonical drift guard for §7.2.

package mcp

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"sort"
	"testing"

	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
)

// expectedP01ToolNames mirrors SPEC.md §5.2.2 (15 P01 tools) in the
// canonical spec order. Kept local to this test so a future spec edit
// that re-orders or adds tools shows up as a single-file diff.
//
// round-16, bead unblock-tv8.71: +promote (Tool 15, the Backlog→Ready
// writer per §6.2 Tool 15 / §6.6) appended at position 15.
var expectedP01ToolNames = []string{
	"prime",
	"ready",
	"claim",
	"create",
	"update",
	"close",
	"show",
	"list",
	"search",
	"comment",
	"add_dependency",
	"remove_dependency",
	"set_state",
	"get_state",
	"promote",
}

// catalogueFileDoc mirrors the on-disk JSON shape — kept independent of
// the generator's internal types so a refactor of cmd/gen-catalogue
// cannot silently change the test invariants.
type catalogueFileDoc struct {
	SchemaVersion string                 `json:"schema_version"`
	Tools         []catalogueFileTool    `json:"tools"`
	Transitions   []json.RawMessage      `json:"transitions"`
	Shared        map[string]interface{} `json:"$shared,omitempty"`
}

type catalogueFileTool struct {
	Name         string          `json:"name"`
	Description  string          `json:"description"`
	InputSchema  json.RawMessage `json:"input_schema"`
	OutputSchema json.RawMessage `json:"output_schema"`
}

func loadCatalogueFromDisk(t *testing.T) catalogueFileDoc {
	t.Helper()
	// _, thisFile, _, _ from runtime.Caller anchors the path
	// resolution to the test file's own directory so the test passes
	// regardless of the working directory the test runner picks
	// (encore test runs from apps/api, plain go test runs from the
	// package directory).
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatalf("runtime.Caller failed; cannot locate catalogue.json")
	}
	path := filepath.Join(filepath.Dir(thisFile), "catalogue.json")
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read catalogue.json: %v", err)
	}
	var doc catalogueFileDoc
	if err := json.Unmarshal(raw, &doc); err != nil {
		t.Fatalf("parse catalogue.json: %v", err)
	}
	return doc
}

// TestCatalogueSchemaVersion pins the schema_version to "v0.1" per
// SPEC §10.3 line 2494. A future bump to "v1" lands with the P02
// BLOCK conditions section.
func TestCatalogueSchemaVersion(t *testing.T) {
	doc := loadCatalogueFromDisk(t)
	if doc.SchemaVersion != "v0.1" {
		t.Errorf("catalogue.json schema_version = %q, want %q", doc.SchemaVersion, "v0.1")
	}
	if CatalogueSchemaVersion != "v0.1" {
		t.Errorf("generated CatalogueSchemaVersion = %q, want %q", CatalogueSchemaVersion, "v0.1")
	}
}

// TestCatalogueTransitionsEmpty pins AC #2: transitions[] is the
// empty array in P01. Populated in P02 alongside the BLOCK conditions.
func TestCatalogueTransitionsEmpty(t *testing.T) {
	doc := loadCatalogueFromDisk(t)
	if len(doc.Transitions) != 0 {
		t.Errorf("catalogue.json transitions[] has %d entries, want 0 (P01 ships empty per SPEC §10.3 line 2504)", len(doc.Transitions))
	}
}

// TestCataloguePxxToolCount pins AC #1 first half: exactly 15 entries
// in tools[] (round-16: +promote). A 16th tool means either a later
// round has shipped (in which case expectedToolCount in the generator +
// this test both bump) or the catalogue and the SPEC have drifted.
func TestCataloguePxxToolCount(t *testing.T) {
	doc := loadCatalogueFromDisk(t)
	if got := len(doc.Tools); got != len(expectedP01ToolNames) {
		t.Errorf("catalogue.json tools[] has %d entries, want %d (SPEC.md §5.2.2 P01 inventory)", got, len(expectedP01ToolNames))
	}
}

// TestCatalogueEachToolHasFourFields pins AC #1 second half: every
// tool entry carries name + description + input_schema + output_schema.
// Empty values fail the structural check inside the generator; this
// test is a belt-and-suspenders guard against a manual edit to the
// committed catalogue.gen.go bypassing the generator.
func TestCatalogueEachToolHasFourFields(t *testing.T) {
	doc := loadCatalogueFromDisk(t)
	for i, tool := range doc.Tools {
		if tool.Name == "" {
			t.Errorf("tools[%d].name is empty", i)
		}
		if tool.Description == "" {
			t.Errorf("tools[%d].description is empty (tool=%q)", i, tool.Name)
		}
		if len(tool.InputSchema) == 0 || bytes.Equal(tool.InputSchema, []byte("null")) {
			t.Errorf("tools[%d].input_schema is empty (tool=%q)", i, tool.Name)
		}
		if len(tool.OutputSchema) == 0 || bytes.Equal(tool.OutputSchema, []byte("null")) {
			t.Errorf("tools[%d].output_schema is empty (tool=%q)", i, tool.Name)
		}
	}
}

// TestCatalogueToolNamesMatchSpecOrder pins the spec-canonical
// ordering of tools[]. The wire ordering matters because the dual-
// location drift check (§7.2) is a byte-level diff between the backend
// catalogue and the Rust plugin's checked-in copy.
func TestCatalogueToolNamesMatchSpecOrder(t *testing.T) {
	doc := loadCatalogueFromDisk(t)
	got := make([]string, 0, len(doc.Tools))
	for _, tool := range doc.Tools {
		got = append(got, tool.Name)
	}
	if !reflect.DeepEqual(got, expectedP01ToolNames) {
		t.Errorf("catalogue.json tool order mismatch:\n  got  = %v\n  want = %v", got, expectedP01ToolNames)
	}
}

// TestCatalogueNamesMatchToolRegistrars asserts that every name in
// catalogue.json is registered against sdkServer in transport.go's
// toolRegistrars table, and vice versa. This is the canonical D-7
// drift guard: the catalogue's tool inventory must be the same set as
// the SDK's live registry. Per the orchestrator's DECISION on this
// bead, the single-source-of-truth assertion is name equality between
// the JSON file and the in-memory sdkServer.
func TestCatalogueNamesMatchToolRegistrars(t *testing.T) {
	doc := loadCatalogueFromDisk(t)
	catalogueNames := make([]string, 0, len(doc.Tools))
	for _, tool := range doc.Tools {
		catalogueNames = append(catalogueNames, tool.Name)
	}

	// sdkServer is the package-level singleton constructed by
	// transport.go::init. We exercise the SDK's tools/list surface
	// over an in-memory transport pair (NewInMemoryTransports) so the
	// assertion mirrors the wire-protocol path agents actually use,
	// not a private package-internal table. This is the canonical
	// drift guard: if a future commit adds a 15th sdkmcp.AddTool call
	// without updating catalogue.json, this test fails.
	registrarNames := liveSDKToolNames(t)

	sort.Strings(catalogueNames)
	sort.Strings(registrarNames)
	if !reflect.DeepEqual(catalogueNames, registrarNames) {
		t.Errorf("catalogue.json tool set differs from sdkServer registration set:\n  catalogue = %v\n  registrar = %v", catalogueNames, registrarNames)
	}
}

// TestGeneratedToolNamesMatchCatalogue asserts the generator's
// ToolNames() helper returns the same names in the same order as the
// source JSON. Catches a stale catalogue.gen.go that survived a
// catalogue.json edit without a fresh `go generate`.
func TestGeneratedToolNamesMatchCatalogue(t *testing.T) {
	doc := loadCatalogueFromDisk(t)
	want := make([]string, 0, len(doc.Tools))
	for _, tool := range doc.Tools {
		want = append(want, tool.Name)
	}
	got := ToolNames()
	if !reflect.DeepEqual(got, want) {
		t.Errorf("ToolNames() mismatch:\n  got  = %v\n  want = %v", got, want)
	}
}

// TestGeneratedToolByNameLookup asserts every name in the catalogue is
// findable via the generated ToolByName helper and an unknown name
// returns ok=false.
func TestGeneratedToolByNameLookup(t *testing.T) {
	for _, name := range expectedP01ToolNames {
		tool, ok := ToolByName(name)
		if !ok {
			t.Errorf("ToolByName(%q) returned ok=false; want present", name)
			continue
		}
		if tool.Name != name {
			t.Errorf("ToolByName(%q).Name = %q, want %q", name, tool.Name, name)
		}
		if tool.Description == "" {
			t.Errorf("ToolByName(%q).Description is empty", name)
		}
		if len(tool.InputSchema) == 0 {
			t.Errorf("ToolByName(%q).InputSchema is empty", name)
		}
		if len(tool.OutputSchema) == 0 {
			t.Errorf("ToolByName(%q).OutputSchema is empty", name)
		}
	}

	if _, ok := ToolByName("does_not_exist"); ok {
		t.Errorf("ToolByName(\"does_not_exist\") returned ok=true; want false")
	}
}

// TestGeneratedCatalogueRoundTrips asserts the generated Catalogue
// []byte parses to the same semantic content as the on-disk JSON.
// Byte-for-byte equality is NOT asserted — the generator canonicalises
// whitespace, so the on-disk file and the embedded constant differ in
// formatting. Semantic equality is what the §7.2 drift CI cares about.
func TestGeneratedCatalogueRoundTrips(t *testing.T) {
	doc := loadCatalogueFromDisk(t)
	wantJSON, err := json.Marshal(doc)
	if err != nil {
		t.Fatalf("marshal disk doc: %v", err)
	}

	var gotDoc catalogueFileDoc
	if err := json.Unmarshal(Catalogue, &gotDoc); err != nil {
		t.Fatalf("unmarshal generated Catalogue: %v", err)
	}
	gotJSON, err := json.Marshal(gotDoc)
	if err != nil {
		t.Fatalf("marshal generated doc: %v", err)
	}

	if !bytes.Equal(wantJSON, gotJSON) {
		t.Errorf("Catalogue []byte does not match catalogue.json semantically.\nRun `go generate ./apps/api/mcp/...` to refresh catalogue.gen.go.\n  disk: %s\n  gen:  %s", wantJSON, gotJSON)
	}
}

// liveSDKToolNames drives the production sdkServer's tools/list surface
// over an in-memory transport pair and returns the tool names the SDK
// actually exposes on the wire. This is the canonical drift guard for
// the catalogue ↔ SDK contract: the catalogue.json + SDK registration
// table MUST agree, and the only way to be sure is to exercise the
// same code path the wire-protocol does.
func liveSDKToolNames(t *testing.T) []string {
	t.Helper()
	ctx := context.Background()

	clientTransport, serverTransport := sdkmcp.NewInMemoryTransports()

	serverSession, err := sdkServer.Connect(ctx, serverTransport, nil)
	if err != nil {
		t.Fatalf("sdkServer.Connect: %v", err)
	}
	t.Cleanup(func() { _ = serverSession.Close() })

	client := sdkmcp.NewClient(&sdkmcp.Implementation{
		Name:    "catalogue-test-client",
		Version: "0.0.0",
	}, nil)
	clientSession, err := client.Connect(ctx, clientTransport, nil)
	if err != nil {
		t.Fatalf("client.Connect: %v", err)
	}
	t.Cleanup(func() { _ = clientSession.Close() })

	res, err := clientSession.ListTools(ctx, nil)
	if err != nil {
		t.Fatalf("clientSession.ListTools: %v", err)
	}

	names := make([]string, 0, len(res.Tools))
	for _, tool := range res.Tools {
		names = append(names, tool.Name)
	}
	return names
}
