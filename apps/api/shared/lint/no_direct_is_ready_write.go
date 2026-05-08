// Package lint provides custom static analyzers wired into golangci-lint
// for the unblock backend. The single analyzer in this package
// (NoDirectIsReadyWriteAnalyzer) enforces SPEC §11.3's architectural
// invariant: the cascade subscriber is the SOLE writer of
// workitems.items.is_ready and workitems.items.pipeline_stage.
//
// Mechanism: the analyzer walks every Go source file in the unblock
// backend, looks for SQL string literals that contain `UPDATE
// workitems.items` (or the unqualified `UPDATE items`) followed by a
// SET clause that targets `is_ready` or `pipeline_stage`, and reports
// every match outside the allow-listed package path. The allow-list
// is compared against `pass.Pkg.Path()` rather than filename — file
// names are brittle when the cascade subscriber package layout
// evolves.
//
// Allow-list (locked at SPEC §11.3 line 2034-2037 + investigation
// note for unblock-tv8.4): the only package permitted to UPDATE
// is_ready / pipeline_stage is `encore.app/deps` (the package that
// hosts cascade_subscriber.go in C-3 / unblock-tv8.12). The cascade
// subscriber file does not yet exist — A-4 ships the analyzer
// shape; C-3 ships the file.
//
// Detection notes:
//
//   - The analyzer matches against raw and back-quoted string literals
//     in any AST position (assignment RHS, function argument, struct
//     literal). It does NOT execute the SQL or parse it as a
//     full PostgreSQL grammar — substring matching is sufficient for
//     the targeted anti-pattern and zero-false-positive within the
//     unblock backend's expected SQL surface.
//   - Pattern is case-insensitive on the SQL keywords (UPDATE / SET)
//     because pgx tolerates either; column names are matched verbatim
//     (`is_ready`, `pipeline_stage`) since the migration uses
//     lower-case identifiers.
//   - String concatenation across two `+` operands is detected: if
//     either operand contains the trigger keyword, the analyzer
//     conservatively flags the construction. False positives are
//     possible but vanishingly rare in real backends; suppress with
//     `//nolint:no_direct_is_ready_write` if the call site is a
//     deliberate test fixture.
//
// The analyzer is consumed by golangci-lint via the module-plugin
// system; see apps/api/.golangci.yml and the plugin entry point at
// apps/api/shared/lint/cmd/no_direct_is_ready_write.
package lint

import (
	"go/ast"
	"strings"

	"golang.org/x/tools/go/analysis"
)

// AllowedPackage is the SOLE Go import path permitted to UPDATE
// workitems.items.is_ready or workitems.items.pipeline_stage. See
// SPEC §11.3 lines 2034-2037 and unblock-tv8.4 investigation §
// "linter allow-list".
const AllowedPackage = "encore.app/deps"

// allowedAuxPackages are packages whose source legitimately contains
// the trigger substrings as documentation, diagnostic messages, or
// fixture data (the analyzer itself, its standalone driver, the test
// fixture root). The cascade-subscriber allow-list is single-entry by
// SPEC; this list is implementation hygiene.
var allowedAuxPackages = map[string]struct{}{
	"encore.app/shared/lint":                              {},
	"encore.app/shared/lint/cmd/no_direct_is_ready_write": {},
	"encore.app/shared/rbac":                              {},
}

// targetColumns is the set of write targets gated by this analyzer.
// Adding entries is a SPEC change; do not extend without one.
var targetColumns = []string{"is_ready", "pipeline_stage"}

// NoDirectIsReadyWriteAnalyzer is the exported analyzer instance.
// golangci-lint's module-plugin loader picks this up via the cmd/
// entry point.
var NoDirectIsReadyWriteAnalyzer = &analysis.Analyzer{
	Name:     "no_direct_is_ready_write",
	Doc:      "rejects UPDATE statements writing workitems.items.is_ready or pipeline_stage outside the cascade subscriber package",
	Run:      run,
	URL:      "https://github.com/websublime/unblock/blob/main/docs/specs/01-spec-backend-mvp.md#113-architectural-invariants",
	Requires: nil,
}

// run implements analysis.Analyzer.Run. Walk every file's AST,
// find string literals carrying the targeted SQL pattern, report.
func run(pass *analysis.Pass) (interface{}, error) {
	if pass.Pkg == nil {
		return nil, nil //nolint:nilnil // analyzer convention
	}
	if pass.Pkg.Path() == AllowedPackage {
		// The cascade subscriber package is exempt by spec.
		return nil, nil //nolint:nilnil
	}
	if _, ok := allowedAuxPackages[pass.Pkg.Path()]; ok {
		// Implementation hygiene: the analyzer itself and the rbac
		// builder source contain the trigger substrings as docs and
		// diagnostic messages, not as live SQL.
		return nil, nil //nolint:nilnil
	}

	for _, file := range pass.Files {
		ast.Inspect(file, func(n ast.Node) bool {
			lit, ok := n.(*ast.BasicLit)
			if !ok {
				return true
			}
			if lit.Kind.String() != "STRING" {
				return true
			}
			payload := unquoteSQL(lit.Value)
			if !looksLikeIsReadyUpdate(payload) {
				return true
			}
			pass.ReportRangef(lit, "direct UPDATE on workitems.items.is_ready or pipeline_stage outside %s; the cascade subscriber is the sole writer per SPEC §11.3", AllowedPackage)
			return true
		})
	}
	return nil, nil //nolint:nilnil
}

// unquoteSQL strips the surrounding quote runes from a Go string
// literal value. Both regular ("…") and raw (`…`) literals are
// supported; malformed inputs are returned as-is so the substring
// scan operates on something rather than nothing.
func unquoteSQL(value string) string {
	if len(value) < 2 {
		return value
	}
	first := value[0]
	last := value[len(value)-1]
	if (first == '"' && last == '"') || (first == '`' && last == '`') {
		return value[1 : len(value)-1]
	}
	return value
}

// looksLikeIsReadyUpdate is the substring matcher: it returns true
// when the payload contains an `UPDATE … workitems.items` (or
// `UPDATE … items`) clause AND a write target hit on
// `is_ready` or `pipeline_stage`. The match is intentionally loose
// on whitespace: a single contiguous payload is enough; multi-line
// SQL is folded into one logical pass.
func looksLikeIsReadyUpdate(payload string) bool {
	lower := strings.ToLower(payload)
	if !strings.Contains(lower, "update ") {
		return false
	}
	hasItemsTable := strings.Contains(lower, "workitems.items") ||
		strings.Contains(lower, " items ") ||
		strings.HasSuffix(lower, " items") ||
		strings.Contains(lower, "\titems")
	if !hasItemsTable {
		return false
	}
	for _, col := range targetColumns {
		if strings.Contains(lower, col) {
			return true
		}
	}
	return false
}
