// Fixture for analysistest: a package whose calls to
// rbac.ScopedQuery.Where (clause, arg 0) and rbac.For (table, arg 1)
// pass non-literal arguments. Every flagged call carries a "want"
// annotation matching the analyzer's diagnostic message.
package rbacclausebad

import (
	"context"
	"fmt"

	"encore.app/shared/rbac"
)

type row struct {
	ID string
}

// -- rbac.ScopedQuery.Where (clause arg, index 0) -----------------------

// Var as the first argument — runtime value, must flag.
func badVar(ctx context.Context, id rbac.Identity, userClause string) ([]row, error) {
	return rbac.For[row](id, "workitems.items").
		Where(userClause). // want `rbac\.Where: first argument must be a Go string literal or untyped string constant`
		Run(ctx)
}

// String concatenation at runtime — must flag. Even when both sides
// look static, Go does not promote `var a, b string; a + b` to a
// constant.
func badConcat(ctx context.Context, id rbac.Identity, suffix string) ([]row, error) {
	return rbac.For[row](id, "workitems.items").
		Where("status = " + suffix). // want `rbac\.Where: first argument must be a Go string literal or untyped string constant`
		Run(ctx)
}

// fmt.Sprintf return — runtime value, must flag.
func badSprintf(ctx context.Context, id rbac.Identity, col string) ([]row, error) {
	return rbac.For[row](id, "workitems.items").
		Where(fmt.Sprintf("%s = $1", col), "Ready"). // want `rbac\.Where: first argument must be a Go string literal or untyped string constant`
		Run(ctx)
}

// Function-return as first argument — runtime value, must flag.
func buildClause() string { return "status = $1" }

func badFuncReturn(ctx context.Context, id rbac.Identity) ([]row, error) {
	return rbac.For[row](id, "workitems.items").
		Where(buildClause(), "Ready"). // want `rbac\.Where: first argument must be a Go string literal or untyped string constant`
		Run(ctx)
}

// Struct-field selector — runtime value, must flag.
type filter struct {
	Clause string
}

func badSelector(ctx context.Context, id rbac.Identity, f filter) ([]row, error) {
	return rbac.For[row](id, "workitems.items").
		Where(f.Clause). // want `rbac\.Where: first argument must be a Go string literal or untyped string constant`
		Run(ctx)
}

// -- rbac.For (table arg, index 1) -------------------------------------
//
// All five shapes mirror the Where bad cases above, but on the SECOND
// positional argument of For. The analyzer must unwrap *ast.IndexExpr
// (the [row] type-argument) to reach the SelectorExpr, then resolve via
// pass.TypesInfo.Uses (NOT Selections — Selections is method-only).

// Var as the table argument — runtime value, must flag.
func badForVarTable(ctx context.Context, id rbac.Identity, userTable string) ([]row, error) {
	return rbac.For[row](id, userTable). // want `rbac\.For: second argument \(table\) must be a Go string literal or untyped string constant`
						Where("status = $1", "Ready").
						Run(ctx)
}

// Runtime concatenation of the table identifier — must flag.
func badForConcatTable(ctx context.Context, id rbac.Identity, schema string) ([]row, error) {
	return rbac.For[row](id, schema+".items"). // want `rbac\.For: second argument \(table\) must be a Go string literal or untyped string constant`
							Where("status = $1", "Ready").
							Run(ctx)
}

// fmt.Sprintf return — runtime value, must flag.
func badForSprintfTable(ctx context.Context, id rbac.Identity, schema string) ([]row, error) {
	return rbac.For[row](id, fmt.Sprintf("%s.items", schema)). // want `rbac\.For: second argument \(table\) must be a Go string literal or untyped string constant`
									Where("status = $1", "Ready").
									Run(ctx)
}

// Function-return as table — runtime value, must flag.
func buildTable() string { return "workitems.items" }

func badForFuncReturnTable(ctx context.Context, id rbac.Identity) ([]row, error) {
	return rbac.For[row](id, buildTable()). // want `rbac\.For: second argument \(table\) must be a Go string literal or untyped string constant`
						Where("status = $1", "Ready").
						Run(ctx)
}

// Struct-field selector as table — runtime value, must flag.
type tableSpec struct {
	Name string
}

func badForSelectorTable(ctx context.Context, id rbac.Identity, t tableSpec) ([]row, error) {
	return rbac.For[row](id, t.Name). // want `rbac\.For: second argument \(table\) must be a Go string literal or untyped string constant`
						Where("status = $1", "Ready").
						Run(ctx)
}
