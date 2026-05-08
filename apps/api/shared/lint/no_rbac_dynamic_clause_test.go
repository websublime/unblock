// Tests for NoRbacDynamicClauseAnalyzer using analysistest.
//
// Three fixtures live under testdata/src/:
//
//   - encore.app/shared/rbac: a stub rbac package matching the locked
//     SPEC §10.1 surface. Imported by both bad/good fixtures so the
//     analyzer's receiver-type resolution sees the canonical package
//     path (`encore.app/shared/rbac`) and matches consistently.
//   - rbacclausebad: every call to rbac.Where with a runtime first
//     argument; each carries a `// want` annotation matching the
//     analyzer's diagnostic.
//   - rbacclausegood: literals, named constants, composed constants,
//     parenthesised literals, and a same-named method on an unrelated
//     receiver type. No diagnostics expected.
//
// The receiver-resolution test in the good fixture (`unrelatedBuilder`
// with its own Where method) is the explicit guard against
// name-only matching: SPEC §10.1's investigation flagged this as a
// MEDIUM risk, and the green-fixture failure mode would be a clear
// signal that the go/types resolution path regressed.
package lint

import (
	"testing"

	"golang.org/x/tools/go/analysis/analysistest"
)

// TestNoRbacDynamicClause_FlagsRuntimeClauses runs the analyzer
// against the rbacclausebad fixture and asserts every `// want`
// annotation is satisfied — vars, BinaryExpr concatenation,
// fmt.Sprintf, function returns, and struct selectors all flag.
func TestNoRbacDynamicClause_FlagsRuntimeClauses(t *testing.T) {
	testdata := analysistest.TestData()
	analysistest.Run(t, testdata, NoRbacDynamicClauseAnalyzer, "rbacclausebad")
}

// TestNoRbacDynamicClause_AllowsCompileTimeConstants runs the analyzer
// against the rbacclausegood fixture and asserts NO diagnostic fires.
// Covers: BasicLit (regular and back-quoted), named const, composed
// const, parenthesised literal, and an unrelated `Where` method on a
// non-rbac receiver type.
func TestNoRbacDynamicClause_AllowsCompileTimeConstants(t *testing.T) {
	testdata := analysistest.TestData()
	analysistest.Run(t, testdata, NoRbacDynamicClauseAnalyzer, "rbacclausegood")
}
