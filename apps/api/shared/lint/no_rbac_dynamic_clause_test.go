// Tests for NoRbacDynamicClauseAnalyzer using analysistest.
//
// Three fixtures live under testdata/src/:
//
//   - encore.app/shared/rbac: a stub rbac package matching the locked
//     SPEC §10.1 surface. Imported by both bad/good fixtures so the
//     analyzer's receiver-type resolution (Where) and Uses-based
//     package-path resolution (For) see the canonical package path
//     (`encore.app/shared/rbac`) and match consistently.
//   - rbacclausebad: every call to rbac.Where with a runtime FIRST
//     argument AND every call to rbac.For with a runtime SECOND
//     argument (table); each carries a `// want` annotation matching
//     the analyzer's diagnostic for the relevant call shape.
//   - rbacclausegood: literals, named constants, composed constants,
//     parenthesised literals (for both Where clause and For table),
//     plus a same-named `Where` method on an unrelated receiver type
//     and a same-named `For` function in an unrelated package. No
//     diagnostics expected.
//
// The resolution-path tests in the good fixture (`unrelatedBuilder`
// with its own Where method, and the local `For` generic in the
// rbacclausegood package) are the explicit guards against name-only
// matching: SPEC §10.1's investigation flagged this as a MEDIUM risk
// for both call shapes. A green-fixture failure mode would be a clear
// signal that either the go/types Selections path (Where) or the
// Uses/IndexExpr-unwrap path (For) regressed.
package lint

import (
	"testing"

	"golang.org/x/tools/go/analysis/analysistest"
)

// TestNoRbacDynamicClause_FlagsRuntimeClauses runs the analyzer
// against the rbacclausebad fixture and asserts every `// want`
// annotation is satisfied. Covers BOTH guarded call shapes:
//
//   - rbac.ScopedQuery.Where (clause arg, index 0): vars, BinaryExpr
//     concatenation, fmt.Sprintf, function returns, struct selectors.
//   - rbac.For (table arg, index 1): same five runtime shapes on the
//     SECOND positional arg. The analyzer must unwrap the *ast.IndexExpr
//     wrapping the type-argument [row] before reaching the SelectorExpr.
func TestNoRbacDynamicClause_FlagsRuntimeClauses(t *testing.T) {
	testdata := analysistest.TestData()
	analysistest.Run(t, testdata, NoRbacDynamicClauseAnalyzer, "rbacclausebad")
}

// TestNoRbacDynamicClause_AllowsCompileTimeConstants runs the analyzer
// against the rbacclausegood fixture and asserts NO diagnostic fires.
// Covers BOTH guarded call shapes for: BasicLit (regular and
// back-quoted), named const, composed const, parenthesised literal,
// plus an unrelated `Where` method on a non-rbac receiver type and an
// unrelated `For` generic function in a non-rbac package.
func TestNoRbacDynamicClause_AllowsCompileTimeConstants(t *testing.T) {
	testdata := analysistest.TestData()
	analysistest.Run(t, testdata, NoRbacDynamicClauseAnalyzer, "rbacclausegood")
}
