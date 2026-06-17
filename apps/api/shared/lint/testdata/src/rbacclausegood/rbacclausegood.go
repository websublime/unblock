// Fixture for analysistest: a package whose calls to
// rbac.ScopedQuery.Where (clause arg) and rbac.For (table arg) pass
// acceptable arguments. The analyzer must NOT fire on any call here
// (no "want" annotations, which analysistest interprets as "expect
// zero diagnostics").
package rbacclausegood

import (
	"context"

	"encore.app/shared/rbac"
)

type row struct {
	ID string
}

// -- rbac.ScopedQuery.Where (clause arg, index 0) -----------------------

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

// -- rbac.For (table arg, index 1) -------------------------------------
//
// Five shapes mirror the Where good cases. Each must NOT flag the
// table argument. The analyzer's resolution path for For uses
// pass.TypesInfo.Uses on the (unwrapped) selector ident; package-path
// gate is `encore.app/shared/rbac`.

// Plain double-quoted string literal as table — canonical form.
func goodForLiteralTable(ctx context.Context, id rbac.Identity) ([]row, error) {
	return rbac.For[row](id, "workitems.items").
		Where("status = $1", "Ready").
		Run(ctx)
}

// Back-quoted (raw) string literal as table.
func goodForRawLiteralTable(ctx context.Context, id rbac.Identity) ([]row, error) {
	return rbac.For[row](id, `deps.dependencies`).
		Where("from_id = $1", "itm_x").
		Run(ctx)
}

// Package-level untyped string constant as table.
const itemsTable = "workitems.items"

func goodForNamedConstTable(ctx context.Context, id rbac.Identity) ([]row, error) {
	return rbac.For[row](id, itemsTable).
		Where("status = $1", "Ready").
		Run(ctx)
}

// Constant expression composed of two string constants as table.
const schemaPart = "workitems"
const tablePart = ".items"
const composedTable = schemaPart + tablePart

func goodForComposedConstTable(ctx context.Context, id rbac.Identity) ([]row, error) {
	return rbac.For[row](id, composedTable).
		Where("status = $1", "Ready").
		Run(ctx)
}

// Parenthesised literal as table — unparen strips the wrapper.
func goodForParenthesisedLiteralTable(ctx context.Context, id rbac.Identity) ([]row, error) {
	return rbac.For[row](id, ("workitems.items")).
		Where("status = $1", "Ready").
		Run(ctx)
}

// A package-level generic `For` function on an UNRELATED package MUST
// NOT false-positive — the analyzer's package-path gate is the
// load-bearing check, name-only matching would otherwise flag any
// `For` symbol elsewhere in the codebase.
//
// The fixture's local For below shares the same name and shape
// (generic, second-arg string) as rbac.For but lives in this package
// — pass.TypesInfo.Uses resolves it to a *types.Func whose Pkg() path
// is `rbacclausegood`, NOT `encore.app/shared/rbac`. The resolution
// gate filters it out.
func For[T any](_ rbac.Identity, _ string) *T { return nil }

func goodUnrelatedFor(id rbac.Identity, userTable string) {
	// userTable is runtime — would be flagged if the analyzer matched
	// by name on the For symbol. Package-path resolution must filter
	// this out.
	_ = For[row](id, userTable)
}

// -- rbac.ScopedQuery.Columns (every variadic column arg) --------------
//
// Each variadic column argument must be a compile-time string constant.
// The shapes mirror the Where/For good cases; the analyzer must NOT
// flag any of them.

// Plain double-quoted string literals — the canonical form.
func goodColumnsLiterals(ctx context.Context, id rbac.Identity) ([]row, error) {
	return rbac.For[row](id, "workitems.items").
		Columns("id", "org_id").
		Where("status = $1", "Ready").
		Run(ctx)
}

// Back-quoted (raw) string literal as a column.
func goodColumnsRawLiteral(ctx context.Context, id rbac.Identity) ([]row, error) {
	return rbac.For[row](id, "workitems.items").
		Columns(`id`).
		Run(ctx)
}

// Package-level untyped string constant — itemsTable is reused as a
// stand-in; a comma-joined column-list const is also a single constant
// (mirrors the production `Columns(itemColumnList)` call shape).
const columnList = "id, org_id, title"

func goodColumnsNamedConst(ctx context.Context, id rbac.Identity) ([]row, error) {
	return rbac.For[row](id, "workitems.items").
		Columns(columnList).
		Run(ctx)
}

// Constant expression composed of two string constants as a column.
const colBase = "org"
const colTail = "_id"
const composedColumn = colBase + colTail

func goodColumnsComposedConst(ctx context.Context, id rbac.Identity) ([]row, error) {
	return rbac.For[row](id, "workitems.items").
		Columns(composedColumn).
		Run(ctx)
}

// Parenthesised literal as a column — unparen strips the wrapper.
func goodColumnsParenthesisedLiteral(ctx context.Context, id rbac.Identity) ([]row, error) {
	return rbac.For[row](id, "workitems.items").
		Columns(("id")).
		Run(ctx)
}

// A `Columns` method on a different type with the same name MUST NOT
// false-positive — receiver-type resolution filters by package path.
func (u *unrelatedBuilder) Columns(_ ...string) *unrelatedBuilder { return u }

func goodUnrelatedColumns(b *unrelatedBuilder, userCol string) {
	// userCol is runtime — would be flagged if the analyzer matched by
	// name. Receiver-type resolution must filter this out.
	b.Columns(userCol)
}
