// gen-catalogue is the codegen entry point for the §10.3 MCP tool
// catalogue. It reads apps/api/mcp/catalogue.json, validates its
// structural invariants (14 tool entries, transitions[] empty, every
// tool carries the four mandatory fields), then emits a deterministic
// catalogue.gen.go file containing:
//
//  1. Catalogue   — the raw JSON bytes (canonicalised via json.Marshal
//     over the parsed tree, so whitespace drift in the
//     source JSON does not produce a diff in the
//     generated file as long as the semantic content is
//     unchanged).
//  2. ToolNames() — []string of tool names in spec order.
//  3. ToolByName(name) (Tool, bool) — lookup by name; returns the typed
//     CatalogueTool with name, description, input_schema, output_schema.
//
// Invocation (via go generate; see apps/api/mcp/catalogue.go):
//
//	go run encore.app/cmd/gen-catalogue \
//	    -in apps/api/mcp/catalogue.json  \
//	    -out apps/api/mcp/catalogue.gen.go
//
// CI drift guard (per bead AC #3 + SPEC §10.3 line 2509):
//
//	go generate ./apps/api/mcp/...
//	git diff --exit-code apps/api/mcp/catalogue.gen.go
//
// Any semantic edit to catalogue.json that is not followed by a fresh
// `go generate` will produce a non-zero diff and fail CI.
//
// This tool is **not** an Encore service. It is a stand-alone CLI under
// apps/api/cmd/ (mirroring the established pattern at
// apps/api/shared/lint/cmd/*/main.go). It has no Encore annotations
// and is excluded from `encore test` / `encore check` by virtue of
// being a `package main` outside any service directory.
package main

import (
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"go/format"
	"os"
	"text/template"
)

// expectedToolCount pins the §10.3 / SPEC.md 5.2.2 P01 inventory at
// 14. A future Pxx revision that adds tools must update this constant
// and the catalogue.json source in the same commit; the structural
// check below is the single boundary that enforces the count.
const expectedToolCount = 14

// catalogueDoc mirrors the top-level JSON shape. Only the fields that
// the generator reasons about structurally appear here; the per-tool
// input_schema / output_schema bodies are kept as json.RawMessage so
// the generator does not need a JSON Schema library to round-trip the
// payload byte-for-byte.
type catalogueDoc struct {
	SchemaVersion string            `json:"schema_version"`
	Tools         []catalogueTool   `json:"tools"`
	Transitions   []json.RawMessage `json:"transitions"`
	Shared        json.RawMessage   `json:"$shared,omitempty"`
}

// catalogueTool mirrors one entry in tools[]. The four required fields
// are validated explicitly; additional fields (none in P01) would be
// preserved verbatim by the round-trip but are not exposed via the
// helper getters.
type catalogueTool struct {
	Name         string          `json:"name"`
	Description  string          `json:"description"`
	InputSchema  json.RawMessage `json:"input_schema"`
	OutputSchema json.RawMessage `json:"output_schema"`
}

func main() {
	in := flag.String("in", "catalogue.json", "path to catalogue.json source")
	out := flag.String("out", "catalogue.gen.go", "path to catalogue.gen.go output")
	flag.Parse()

	raw, err := os.ReadFile(*in)
	if err != nil {
		die("read %s: %v", *in, err)
	}

	var doc catalogueDoc
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.DisallowUnknownFields()
	if err := dec.Decode(&doc); err != nil {
		die("parse %s: %v", *in, err)
	}

	// Structural invariants (bead AC #1, #2, #3).
	if doc.SchemaVersion == "" {
		die("schema_version is empty")
	}
	if got := len(doc.Tools); got != expectedToolCount {
		die("tools[] has %d entries, want exactly %d (P01 inventory; see SPEC.md §5.2.2)", got, expectedToolCount)
	}
	if len(doc.Transitions) != 0 {
		die("transitions[] must be empty in P01 (got %d entries); populated in P02 per SPEC §10.3 line 2504", len(doc.Transitions))
	}
	for i, t := range doc.Tools {
		if t.Name == "" {
			die("tools[%d].name is empty", i)
		}
		if t.Description == "" {
			die("tools[%d].description is empty (tool=%q)", i, t.Name)
		}
		if len(t.InputSchema) == 0 || string(t.InputSchema) == "null" {
			die("tools[%d].input_schema is empty (tool=%q)", i, t.Name)
		}
		if len(t.OutputSchema) == 0 || string(t.OutputSchema) == "null" {
			die("tools[%d].output_schema is empty (tool=%q)", i, t.Name)
		}
	}

	// Canonicalise the raw bytes via a re-marshal so whitespace drift
	// in the source file does not produce a diff in the generated
	// constant. The Catalogue []byte value emitted below is the
	// canonical wire form; the on-disk catalogue.json remains the
	// human-editable source.
	canonical, err := json.Marshal(doc)
	if err != nil {
		die("re-marshal canonical bytes: %v", err)
	}

	// Build per-tool entries for the ToolByName lookup table. We embed
	// the raw schemas as []byte so callers can json.Unmarshal them into
	// any client-side type at the call site without paying the cost
	// here.
	tmplData := struct {
		Doc       catalogueDoc
		Canonical string
		Tools     []toolTemplateEntry
	}{
		Doc:       doc,
		Canonical: goBytesLiteral(canonical),
		Tools:     make([]toolTemplateEntry, 0, len(doc.Tools)),
	}
	for _, t := range doc.Tools {
		// Pre-canonicalise each tool's schemas via a round-trip so the
		// generated bytes are stable independent of source whitespace.
		var inSchema, outSchema any
		if err := json.Unmarshal(t.InputSchema, &inSchema); err != nil {
			die("tool %q: input_schema is not valid JSON: %v", t.Name, err)
		}
		if err := json.Unmarshal(t.OutputSchema, &outSchema); err != nil {
			die("tool %q: output_schema is not valid JSON: %v", t.Name, err)
		}
		inCanonical, err := json.Marshal(inSchema)
		if err != nil {
			die("tool %q: re-marshal input_schema: %v", t.Name, err)
		}
		outCanonical, err := json.Marshal(outSchema)
		if err != nil {
			die("tool %q: re-marshal output_schema: %v", t.Name, err)
		}
		tmplData.Tools = append(tmplData.Tools, toolTemplateEntry{
			Name:         t.Name,
			Description:  t.Description,
			InputSchema:  goBytesLiteral(inCanonical),
			OutputSchema: goBytesLiteral(outCanonical),
		})
	}

	var buf bytes.Buffer
	if err := tmpl.Execute(&buf, tmplData); err != nil {
		die("execute template: %v", err)
	}

	formatted, err := format.Source(buf.Bytes())
	if err != nil {
		die("gofmt: %v\n--- buffer ---\n%s", err, buf.String())
	}

	if err := os.WriteFile(*out, formatted, 0o644); err != nil {
		die("write %s: %v", *out, err)
	}
}

func die(f string, a ...any) {
	fmt.Fprintf(os.Stderr, "gen-catalogue: "+f+"\n", a...)
	os.Exit(1)
}

// goBytesLiteral renders a []byte as a Go source `[]byte("…")` literal
// with each byte escaped via strconv.Quote-style rules. We use the
// strconv-equivalent escape for printable ASCII and \xNN for the rest
// — Go's default %q would work but emits a string, not a []byte
// literal. Doing it explicitly keeps the generated file readable on
// review.
func goBytesLiteral(b []byte) string {
	// Easiest correct path: serialise as a Go double-quoted string
	// (strconv.Quote semantics) then prefix with []byte. The runtime
	// representation is identical to a []byte literal and the source
	// stays diff-friendly.
	return fmt.Sprintf("[]byte(%s)", quoteForGo(b))
}

// quoteForGo returns a Go-syntax double-quoted string for b. We use
// %q which honors UTF-8 and escapes control characters; the resulting
// literal compiles to a string with the exact bytes of b when b is
// valid UTF-8 (always true for JSON output of encoding/json).
func quoteForGo(b []byte) string {
	return fmt.Sprintf("%q", string(b))
}

type toolTemplateEntry struct {
	Name         string
	Description  string
	InputSchema  string
	OutputSchema string
}

// tmpl is the catalogue.gen.go output template. The leading
// `// Code generated …; DO NOT EDIT.` banner is the canonical Go
// signal that downstream tooling (`go generate`, gofmt, golangci-lint)
// uses to identify generated sources.
var tmpl = template.Must(template.New("catalogue.gen.go").Parse(`// Code generated by apps/api/cmd/gen-catalogue; DO NOT EDIT.
//
// Source: apps/api/mcp/catalogue.json
// Regenerate: cd apps/api && go generate ./mcp/...
//
// The Catalogue []byte value is the canonicalised JSON of the source
// file (whitespace-normalised). ToolNames() returns the tool names in
// spec order. ToolByName(name) returns the typed CatalogueTool for a
// given tool name; the input_schema and output_schema fields are
// pre-canonicalised []byte payloads that callers may json.Unmarshal
// into any client-side type.
//
// SPEC anchors:
//   - §10.3 (catalogue authoring + go generate wiring)
//   - SPEC.md §7.2 (dual-location catalogue contract; D-7 ships the
//     backend half, the Rust unblock-plugin half lands in P04)
//   - SPEC.md §5.2.2 (14 P01 tools inventory)

package mcp

// CatalogueSchemaVersion mirrors the schema_version field at the top of
// catalogue.json. P01 ships at "v0.1"; P02 will bump to "v1" when the
// BLOCK conditions section lands and the transitions[] array is
// populated.
const CatalogueSchemaVersion = {{ printf "%q" .Doc.SchemaVersion }}

// Catalogue is the canonical wire-form JSON of the §10.3 tool
// catalogue. Whitespace is normalised relative to the source file via
// json.Marshal round-trip; the semantic content matches
// apps/api/mcp/catalogue.json byte-for-byte under the round-trip.
var Catalogue = {{ .Canonical }}

// CatalogueTool is one entry in the catalogue.tools[] array. The
// InputSchema and OutputSchema fields carry the raw JSON Schema as
// []byte; callers that need a typed representation should
// json.Unmarshal into a JSON Schema struct of their choice.
type CatalogueTool struct {
	Name         string
	Description  string
	InputSchema  []byte
	OutputSchema []byte
}

// catalogueTools is the typed table of all P01 tools in spec order.
// Kept private so the public surface is the two helper functions
// below; future revisions can change the storage shape without
// breaking callers.
var catalogueTools = []CatalogueTool{
{{- range .Tools }}
	{
		Name:         {{ printf "%q" .Name }},
		Description:  {{ printf "%q" .Description }},
		InputSchema:  {{ .InputSchema }},
		OutputSchema: {{ .OutputSchema }},
	},
{{- end }}
}

// catalogueToolsByName is the lookup index built once at package init.
// O(1) lookups by tool name with no allocation on the hot path.
var catalogueToolsByName = func() map[string]CatalogueTool {
	m := make(map[string]CatalogueTool, len(catalogueTools))
	for _, t := range catalogueTools {
		m[t.Name] = t
	}
	return m
}()

// ToolNames returns the catalogue tool names in spec order. The
// returned slice is a fresh copy — callers may mutate it freely
// without affecting the package-internal table.
func ToolNames() []string {
	out := make([]string, len(catalogueTools))
	for i, t := range catalogueTools {
		out[i] = t.Name
	}
	return out
}

// ToolByName returns the CatalogueTool for the given name. The second
// return is false when no tool with that name exists in the catalogue.
func ToolByName(name string) (CatalogueTool, bool) {
	t, ok := catalogueToolsByName[name]
	return t, ok
}
`))
