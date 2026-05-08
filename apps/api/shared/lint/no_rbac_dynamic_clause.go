// no_rbac_dynamic_clause.go — second project-local analyzer for the
// unblock backend. Enforces SPEC §10.1's injection-safety invariant on
// the rbac typed query builder: the first argument to
// `(*encore.app/shared/rbac.ScopedQuery[T]).Where` MUST be a compile-time
// string constant. Every runtime value flows through the `args...`
// variadic; the clause text is never user-controlled.
//
// Why a static analyzer (and not a runtime check, and not a wrapper
// type):
//
//   - Runtime check: by the time the SQL string reaches the rbac
//     builder's build() helper, Go has erased the distinction between
//     a literal and a runtime-constructed string. The string is
//     opaque. Runtime validation against arbitrary SQL meta-characters
//     ('--', ';', '/*', '%') is brittle and, for a
//     tenant-isolation gate, fundamentally not safe enough — a single
//     missed escape leaks across orgs. The only correctness-preserving
//     gate is "no runtime construction permitted at all", which is a
//     compile-time property the analyzer enforces.
//   - Wrapper type (a `SafeClause string` type with package-private
//     constructor): would require a SPEC §10.1 amendment because it
//     changes the locked Where signature. Investigation
//     (unblock-tv8.33) explicitly recommended the analyzer path
//     instead.
//
// Detection rules (the only acceptable forms for the FIRST argument):
//
//   - `*ast.BasicLit` of `token.STRING` kind — covers regular ("…") and
//     back-quoted (`…`) Go string literals.
//   - A typed/untyped *types.Const of string kind — covers references
//     to a package-level `const` whose initialiser is itself a literal
//     or a constant expression. The Go compiler folds string constants
//     at compile time, so the result is byte-fixed before the analyzer
//     ever runs.
//
// Everything else is rejected:
//
//   - `*ast.Ident` referencing a `var` (runtime-mutable).
//   - `*ast.BinaryExpr` with `+` operator (runtime concatenation, even
//     when both operands look static — Go does not promote `var a, b
//     string; a + b` to a constant).
//   - `*ast.CallExpr` (fmt.Sprintf, strings.Join, helper functions).
//   - `*ast.SelectorExpr` resolving to a non-const (struct field, etc).
//   - `*ast.ParenExpr` is unwrapped recursively before the rules above
//     are applied; everything else passes through verbatim and fails
//     the "is constant" check.
//
// Receiver-type resolution: the analyzer uses `pass.TypesInfo.Selections`
// to extract the receiver's type, then walks through *types.Pointer and
// *types.Named to land on the underlying generic type. The match
// requires (a) the receiver type's package import path is exactly
// `encore.app/shared/rbac` and (b) the method's name is `Where`.
// Name-only matching is explicitly avoided — a `Where` method on an
// unrelated query builder elsewhere in the backend would otherwise
// false-positive.
package lint

import (
	"go/ast"
	"go/constant"
	"go/token"
	"go/types"

	"golang.org/x/tools/go/analysis"
)

// rbacPackagePath is the canonical Go import path for the rbac typed
// query builder package. The analyzer uses pass.TypesInfo to resolve
// the receiver type back to this path before flagging — name-only
// matching ('any method called Where') would false-positive on
// unrelated query builders.
const rbacPackagePath = "encore.app/shared/rbac"

// rbacWhereMethod is the locked SPEC §10.1 method name on
// rbac.ScopedQuery[T] whose first argument the analyzer guards.
const rbacWhereMethod = "Where"

// NoRbacDynamicClauseAnalyzer is the exported analyzer instance. The
// golangci-lint module-plugin loader picks this up via the cmd/
// entry point.
var NoRbacDynamicClauseAnalyzer = &analysis.Analyzer{
	Name:     "no_rbac_dynamic_clause",
	Doc:      "rejects runtime-constructed first arguments to rbac.ScopedQuery.Where; clause must be a Go string literal or untyped string constant (SPEC §10.1, unblock-tv8.33)",
	Run:      runNoRbacDynamicClause,
	URL:      "https://github.com/websublime/unblock/blob/main/docs/specs/01-spec-backend-mvp.md#101-rbac-pkgrbac-nfr-2",
	Requires: nil,
}

// runNoRbacDynamicClause implements analysis.Analyzer.Run. Walks every
// CallExpr in every file, filters to calls whose selector resolves to
// (*encore.app/shared/rbac.ScopedQuery[T]).Where via go/types, and
// verifies the FIRST argument is a compile-time string constant.
func runNoRbacDynamicClause(pass *analysis.Pass) (interface{}, error) {
	if pass.Pkg == nil {
		return nil, nil //nolint:nilnil // analyzer convention
	}
	// The rbac package's own tests call Where with literals, but they
	// also exercise internal helpers. Skip the package itself so the
	// analyzer never analyses its own subject — false positives would
	// be confusing and the package's own correctness is governed by
	// the doc-hardening pass (rbac.go SECURITY block on Where).
	if pass.Pkg.Path() == rbacPackagePath {
		return nil, nil //nolint:nilnil
	}

	for _, file := range pass.Files {
		ast.Inspect(file, func(n ast.Node) bool {
			call, ok := n.(*ast.CallExpr)
			if !ok {
				return true
			}
			sel, ok := call.Fun.(*ast.SelectorExpr)
			if !ok {
				return true
			}
			if sel.Sel == nil || sel.Sel.Name != rbacWhereMethod {
				return true
			}
			if !isRbacScopedQueryReceiver(pass.TypesInfo, sel) {
				return true
			}
			if len(call.Args) == 0 {
				// Wrong arity — go/types will complain elsewhere; this
				// analyzer is only concerned with the safety contract.
				return true
			}
			first := call.Args[0]
			if isCompileTimeStringConstant(pass.TypesInfo, first) {
				return true
			}
			pass.ReportRangef(first, "rbac.Where: first argument must be a Go string literal or untyped string constant; runtime values MUST flow through args... — see SPEC §10.1 / unblock-tv8.33")
			return true
		})
	}
	return nil, nil //nolint:nilnil
}

// isRbacScopedQueryReceiver returns true when sel resolves to a method
// on a *encore.app/shared/rbac.ScopedQuery[T] receiver. The analyzer
// must reject false positives on unrelated query builders, so we walk
// through the type-info graph rather than match by name alone.
func isRbacScopedQueryReceiver(info *types.Info, sel *ast.SelectorExpr) bool {
	if info == nil {
		return false
	}
	// Selections records selectors that resolve to a method or field
	// on a typed expression (e.g. `q.Where(...)` where q is typed).
	// Package-qualified identifiers (e.g. `pkg.Func`) live in
	// info.Uses instead, but rbac.Where is always a method call so
	// Selections is the right map.
	selection, ok := info.Selections[sel]
	if !ok {
		return false
	}
	recv := selection.Recv()
	if recv == nil {
		return false
	}
	named := unwrapToNamed(recv)
	if named == nil {
		return false
	}
	obj := named.Obj()
	if obj == nil || obj.Pkg() == nil {
		return false
	}
	return obj.Pkg().Path() == rbacPackagePath
}

// unwrapToNamed strips *types.Pointer wrappers and unwraps generic
// instantiations down to the underlying *types.Named. Go represents
// `*ScopedQuery[T]` as Pointer(Named(ScopedQuery)) at the type-info
// level; method receivers may be either pointer or value, so we
// accept both.
func unwrapToNamed(t types.Type) *types.Named {
	for {
		switch tt := t.(type) {
		case *types.Pointer:
			t = tt.Elem()
		case *types.Named:
			return tt
		case *types.Alias:
			// Go 1.22+ exposes type aliases as *types.Alias; unwrap to
			// the aliased type so an `type X = rbac.ScopedQuery[T]`
			// declaration still resolves correctly.
			t = types.Unalias(tt)
		default:
			return nil
		}
	}
}

// isCompileTimeStringConstant decides whether expr is acceptable as
// the FIRST argument to rbac.Where. Two acceptable shapes:
//
//   - *ast.BasicLit of token.STRING — a literal string written
//     directly at the call site (regular or back-quoted).
//   - Any expression whose go/types Constant value is non-nil and of
//     string kind — covers package-level `const x = "…"` and
//     compile-time-foldable expressions like `const x = a + b` where a
//     and b are themselves string constants.
//
// Everything else (runtime vars, function returns, BinaryExpr on
// non-constants, type conversions on non-constants) returns false.
func isCompileTimeStringConstant(info *types.Info, expr ast.Expr) bool {
	expr = unparen(expr)

	if lit, ok := expr.(*ast.BasicLit); ok {
		return lit.Kind == token.STRING
	}
	if info == nil {
		return false
	}
	tv, ok := info.Types[expr]
	if !ok {
		return false
	}
	if tv.Value == nil {
		// Non-constant expression. go/types only populates Value when
		// the expression evaluates to a compile-time constant; a var
		// reference, function call, or BinaryExpr on non-constants
		// leaves Value nil.
		return false
	}
	return tv.Value.Kind() == constant.String
}

// unparen strips redundant *ast.ParenExpr wrappers. A user might write
// `q.Where(("status = $1"), ...)`; the analyzer should treat that the
// same as the bare literal.
func unparen(expr ast.Expr) ast.Expr {
	for {
		paren, ok := expr.(*ast.ParenExpr)
		if !ok {
			return expr
		}
		expr = paren.X
	}
}
