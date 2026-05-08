// Fake encore.app/shared/rbac package for analysistest fixtures.
//
// analysistest builds a self-contained module rooted at testdata/src/,
// so the real rbac package (which imports encore.dev/storage/sqldb and
// encore.app/auth) cannot be referenced — pulling those in would
// require importing the entire Encore runtime into the lint testdata
// tree. Instead, this fake mirrors the locked SPEC §10.1 surface
// (For/Where/Run signatures plus the ScopedQuery[T] type) so the
// analyzer's go/types-based receiver resolution sees the same package
// path (`encore.app/shared/rbac`) and matches the same way it does
// against the real package.
//
// Bodies are stubs — the analyzer never executes the code, only walks
// its types. The methods return the receiver so fluent chains in the
// fixture compile without runtime semantics.
package rbac

import "context"

// ScopedQuery is the public type the analyzer's receiver-resolution
// logic must recognise. The internal layout doesn't matter to the
// analyzer; only the package path of the named type does.
type ScopedQuery[T any] struct {
	_ T
}

// Identity is a stand-in for auth.Identity; the analyzer doesn't
// inspect it.
type Identity struct {
	OrgID string
}

// For mirrors rbac.For so fixtures can build a *ScopedQuery[T] for
// chaining into Where calls.
func For[T any](_ Identity, _ string) *ScopedQuery[T] {
	return &ScopedQuery[T]{}
}

// Where mirrors the locked SPEC §10.1 signature. The analyzer flags
// non-literal first arguments at call sites of THIS method.
func (q *ScopedQuery[T]) Where(_ string, _ ...any) *ScopedQuery[T] {
	return q
}

// Run mirrors the locked SPEC §10.1 signature so chained call sites
// compile.
func (q *ScopedQuery[T]) Run(_ context.Context) ([]T, error) {
	return nil, nil
}
