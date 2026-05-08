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
//     full PostgreSQL grammar — a single regex anchored on the
//     UPDATE…SET…<col> shape is sufficient for the targeted
//     anti-pattern and rejects substring false positives (SELECT
//     clauses returning is_ready, comment fragments mentioning
//     "update items.is_ready", INSERT statements listing the column,
//     adjacent identifiers like update_items_at).
//   - Pattern is case-insensitive on every component (UPDATE / SET /
//     items / column name) because pgx tolerates mixed casing and
//     gofmt/golint-driven SQL hand-formatting in this backend has
//     historically used both. Column names are still drawn from the
//     `targetColumns` slice — the regex builds the alternation from
//     that slice at package init, so adding a future spec'd column
//     does not require touching the regex literal.
//   - The (?s) (dotall) flag lets the regex span embedded newlines,
//     so multi-line UPDATE…SET payloads (the common pgx style) match
//     in a single pass without manual whitespace folding.
//   - The regex caps both the UPDATE→items→SET and SET→column gaps
//     with `[^;]*?`, so a literal carrying multiple statements does
//     not stitch a forbidden pattern across an unrelated statement
//     boundary.
//   - String concatenation across two `+` operands is NOT detected:
//     this analyzer walks `*ast.BasicLit` nodes only, so a
//     deliberately split UPDATE statement bypasses it. The cost of
//     adding BinaryExpr coverage (false positives on string-builder
//     code that happens to contain "UPDATE" and "is_ready" in
//     unrelated literals) outweighs the benefit; that pattern is
//     out-of-scope for this analyzer.
//   - SQL comments (`--` line comments, `/* … */` block comments) are
//     NOT parsed: the analyzer scans raw BasicLit text without a SQL
//     lexer, so a string literal containing a syntactically complete
//     fake UPDATE inside a comment (e.g. `-- UPDATE workitems.items
//     SET is_ready = true`) will still false-positive. This is a
//     known-and-accepted residual: comment-aware parsing would
//     require a real SQL grammar, the false-positive cost of the
//     current heuristic is bounded (literals carrying full fake DML
//     in comments are vanishingly rare in production code), and the
//     escape hatch is to relocate the example out of an executable
//     literal (e.g. into a Go-source `//` comment, which the analyzer
//     does not scan).
//
// The analyzer is consumed by golangci-lint via the module-plugin
// system; see apps/api/.golangci.yml and the plugin entry point at
// apps/api/shared/lint/cmd/no_direct_is_ready_write.
package lint

import (
	"go/ast"
	"regexp"
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

// isReadyUpdateRe matches an UPDATE statement on workitems.items (or
// the unqualified `items` table) whose SET clause writes one of the
// targetColumns columns. Anatomy:
//
//   - (?is) — case-insensitive (?i) for SQL keywords + identifiers,
//     dotall (?s) so the inner [^;]*? can span embedded newlines for
//     multi-line UPDATE…SET payloads.
//   - \bupdate — the UPDATE keyword, word-bounded so identifiers
//     like `update_items_at` do not match.
//   - (?:\s|\\.)+ — one-or-more whitespace OR Go escape sequence
//     (backslash + any char). This lets the analyzer match SQL
//     authored as a regular Go string literal like
//     `"UPDATE\tworkitems.items SET ..."` where the payload contains
//     the literal two-character sequence `\t` rather than a real
//     TAB; analysistest passes the unprocessed literal text through
//     `unquoteSQL`, so escape bytes survive verbatim.
//   - (?:workitems\.items|items)\b — explicit alternation of the
//     two accepted table forms: schema-qualified `workitems.items`
//     or the bare `items` table. Written as an explicit alternation
//     rather than the equivalent factored form `(?:workitems\.)?items`
//     so the segment self-documents the closed set of accepted
//     prefixes — only `workitems.` is allowed; an unrelated schema
//     like `auth.items` cannot land on the bare branch because the
//     preceding `(?:\s|\\.)+` separator does not span letters, so
//     after a non-`workitems` schema-qualifier the regex has no path
//     forward. The trailing \b rejects sibling tables like
//     `dependency_items`.
//   - [^;]*?\bset\b — guarded gap to the SET keyword; the [^;]
//     character class prevents the regex from stitching across an
//     unrelated statement that happens to be in the same literal.
//   - [^;]*?\b(<col1>|<col2>)\b — final guarded gap to one of the
//     targetColumns column names; the alternation is built from the
//     targetColumns slice at package init via regexp.QuoteMeta.
//
// The regex is compiled once at package load. A per-call compile
// would be wasted on every literal walked by the analyzer.
var isReadyUpdateRe = buildIsReadyUpdateRe(targetColumns)

// buildIsReadyUpdateRe assembles the UPDATE…SET…<col> regex from a
// dynamic column list. Exposed as a function so the package init has
// a single, testable construction point and so a future SPEC-driven
// column addition is a one-line change to targetColumns.
func buildIsReadyUpdateRe(columns []string) *regexp.Regexp {
	if len(columns) == 0 {
		// Defensive: an empty column list would compile to a regex
		// that matches every UPDATE statement. The analyzer's contract
		// is "match these exact columns or nothing"; refuse to compile
		// a pattern with no anchors.
		panic("lint: targetColumns must not be empty — see SPEC §11.3")
	}
	quoted := make([]string, len(columns))
	for i, col := range columns {
		quoted[i] = regexp.QuoteMeta(col)
	}
	pattern := `(?is)\bupdate(?:\s|\\.)+(?:workitems\.items|items)\b[^;]*?\bset\b[^;]*?\b(?:` +
		strings.Join(quoted, "|") +
		`)\b`
	return regexp.MustCompile(pattern)
}

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

// looksLikeIsReadyUpdate is the regex-anchored matcher: it returns
// true when the payload contains an UPDATE statement on workitems.items
// (or the bare `items` table) whose SET clause writes one of the
// targetColumns columns. The regex is built once at package init from
// the targetColumns slice; this function is a thin wrapper so the
// run() loop reads naturally and the package surface stays stable.
func looksLikeIsReadyUpdate(payload string) bool {
	return isReadyUpdateRe.MatchString(payload)
}
