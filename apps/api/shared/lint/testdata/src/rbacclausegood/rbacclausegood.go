// Fixture for analysistest: a package whose calls to
// rbac.ScopedQuery.Where pass acceptable first arguments. The
// analyzer must NOT fire on any call here (no "want" annotations,
// which analysistest interprets as "expect zero diagnostics").
package rbacclausegood

import (
	"context"

	"encore.app/shared/rbac"
)

type row struct {
	ID string
}

// Plain double-quoted string literal — the canonical form.
func goodLiteral(ctx context.Context, id rbac.Identity) ([]row, error) {
	return rbac.For[row](id, "workitems.items").
		Where("status = $1", "Ready").
		Run(ctx)
}

// Back-quoted (raw) string literal — equivalent for our purposes.
func goodRawLiteral(ctx context.Context, id rbac.Identity) ([]row, error) {
	return rbac.For[row](id, "workitems.items").
		Where(`status = $1 AND priority = $2`, "Ready", "P0").
		Run(ctx)
}

// Package-level untyped string constant — go/types resolves this to a
// constant value at compile time.
const filterClause = "status = $1 AND is_archived = false"

func goodNamedConst(ctx context.Context, id rbac.Identity) ([]row, error) {
	return rbac.For[row](id, "workitems.items").
		Where(filterClause, "Ready").
		Run(ctx)
}

// Constant expression composed of two string constants — Go folds
// these at compile time, so the result is byte-fixed before the
// analyzer ever runs.
const baseClause = "status = $1"
const tailClause = " AND is_archived = false"
const composedClause = baseClause + tailClause

func goodComposedConst(ctx context.Context, id rbac.Identity) ([]row, error) {
	return rbac.For[row](id, "workitems.items").
		Where(composedClause, "Ready").
		Run(ctx)
}

// Parenthesised literal — `unparen` strips the wrapper, the bare
// literal still passes the BasicLit check.
func goodParenthesisedLiteral(ctx context.Context, id rbac.Identity) ([]row, error) {
	return rbac.For[row](id, "workitems.items").
		Where(("status = $1"), "Ready").
		Run(ctx)
}

// A `Where` method on a different type with the same name MUST NOT
// false-positive — the analyzer's receiver-resolution gate filters by
// package path, not method name alone.
type unrelatedBuilder struct{}

func (u *unrelatedBuilder) Where(_ string, _ ...any) *unrelatedBuilder { return u }

func goodUnrelatedWhere(b *unrelatedBuilder, userClause string) {
	// userClause is runtime — would be flagged if the analyzer
	// matched by name. Receiver-type resolution must filter this out.
	b.Where(userClause)
}
