// Fixture for analysistest: a package whose calls to
// rbac.ScopedQuery.Where pass non-literal first arguments. Every
// flagged call carries a "want" annotation matching the analyzer's
// diagnostic message.
package rbacclausebad

import (
	"context"
	"fmt"

	"encore.app/shared/rbac"
)

type row struct {
	ID string
}

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
