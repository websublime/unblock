// no_rbac_dynamic_clause.go — second project-local analyzer for the
// unblock backend. Enforces SPEC §10.1's injection-safety invariant on
// the rbac typed query builder against THREE call shapes:
//
//   - `(*encore.app/shared/rbac.ScopedQuery[T]).Where(clause, args...)`
//     — first argument (clause, index 0) MUST be a compile-time string
//     constant (unblock-tv8.33).
//   - `encore.app/shared/rbac.For[T](identity, table)` — second
//     argument (table, index 1) MUST be a compile-time string constant
//     (unblock-tv8.35). The table identifier is concatenated verbatim
//     into the FROM clause AND interpolated into the canonical scope
//     predicate (`<table>.org_id = $1`); a runtime value rewrites both
//     sinks and silently bypasses tenant isolation, exactly the same
//     class of footgun Where guards against.
//   - `(*encore.app/shared/rbac.ScopedQuery[T]).Columns(cols...)` —
//     EVERY variadic argument (each column identifier) MUST be a
//     compile-time string constant (unblock-8xb.8). Each identifier is
//     concatenated verbatim into the SELECT projection; like the table,
//     a SQL identifier has no bind channel, so a runtime column value is
//     an injection sink. Round-17 / SPEC §11.3 extends the invariant to
//     this third sink.
//
// Every runtime value flows through args... (Where) or is forbidden
// outright (For's table and Columns' identifiers have no positional bind
// channel — they are SQL identifiers, not values).
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
//   - Wrapper type (a `SafeClause string` or `SafeTable string` type
//     with package-private constructor): would require a SPEC §10.1
//     amendment because it changes the locked Where/For signature.
//     Investigation (unblock-tv8.33, unblock-tv8.35) explicitly
//     recommended the analyzer path instead.
//
// Detection rules (the only acceptable forms for the guarded argument):
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
// Resolution paths (two distinct go/types maps):
//
//   - Where (method-call): `pass.TypesInfo.Selections` records selectors
//     that resolve to a method or field on a typed receiver. The
//     analyzer walks Selection.Recv() through *types.Pointer / *types.Named
//     to confirm the receiver's package path is `encore.app/shared/rbac`
//     and method name is `Where`. Name-only matching is explicitly
//     avoided — a `Where` method on an unrelated query builder elsewhere
//     in the backend would otherwise false-positive.
//   - For (package-level generic func): `Selections` does NOT capture
//     package-qualified identifier resolution; `pass.TypesInfo.Uses`
//     (equivalently `ObjectOf`) is the correct map. The analyzer
//     resolves the SelectorExpr's selector ident to a *types.Func and
//     asserts its package path is `encore.app/shared/rbac` and name is
//     `For`. Critical AST nuance: `rbac.For[row](id, table)` parses as
//     CallExpr{Fun: IndexExpr{X: SelectorExpr{rbac.For}, Index: row},
//     Args: [id, table]} (single type arg) or IndexListExpr (multiple
//     type args, Go 1.18+). The walker MUST unwrap *ast.IndexExpr and
//     *ast.IndexListExpr before reaching the SelectorExpr; a naive
//     match on call.Fun.(*ast.SelectorExpr) silently misses every For
//     call site because the IndexExpr wraps it.
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

// rbacColumnsMethod is the SPEC §10.1 method name on rbac.ScopedQuery[T]
// (round-17, unblock-8xb.8) whose EVERY variadic argument the analyzer
// guards. Like Where it is a method on the typed receiver, so it
// resolves through pass.TypesInfo.Selections.
const rbacColumnsMethod = "Columns"

// rbacForFunc is the locked SPEC §10.1 package-level constructor name on
// rbac whose SECOND argument (table) the analyzer guards. For is
// generic, so the AST shape at the call site is
// CallExpr{Fun: IndexExpr|IndexListExpr{X: SelectorExpr{rbac.For}}}.
const rbacForFunc = "For"

// NoRbacDynamicClauseAnalyzer is the exported analyzer instance. The
// golangci-lint module-plugin loader picks this up via the cmd/
// entry point. Despite the file/registration name retaining the
// historical "clause" suffix (kept stable to avoid churning the
// golangci-lint custom-binary registration), the analyzer guards
// `rbac.ScopedQuery.Where` (clause, arg index 0), `rbac.For` (table,
// arg index 1), and `rbac.ScopedQuery.Columns` (every variadic column
// arg). The Doc string is the authoritative scope.
var NoRbacDynamicClauseAnalyzer = &analysis.Analyzer{
	Name:     "no_rbac_dynamic_clause",
	Doc:      "rejects runtime-constructed string arguments to rbac.ScopedQuery.Where (clause, arg 0), rbac.For (table, arg 1), and rbac.ScopedQuery.Columns (every variadic column); all MUST be a Go string literal or untyped string constant (SPEC §10.1/§11.3, unblock-tv8.33, unblock-tv8.35, unblock-8xb.8)",
	Run:      runNoRbacDynamicClause,
	URL:      "https://github.com/websublime/unblock/blob/main/docs/specs/01-spec-backend-mvp.md#101-rbac-pkgrbac-nfr-2",
	Requires: nil,
}

// runNoRbacDynamicClause implements analysis.Analyzer.Run. Walks every
// CallExpr in every file and applies two parallel detection paths:
//
//   - Method calls whose selector resolves to
//     (*encore.app/shared/rbac.ScopedQuery[T]).Where via
//     pass.TypesInfo.Selections — guards arg index 0 (clause).
//   - Package-level generic calls whose selector resolves (after
//     unwrapping *ast.IndexExpr / *ast.IndexListExpr for the type-arg
//     wrapper) to encore.app/shared/rbac.For via pass.TypesInfo.Uses —
//     guards arg index 1 (table).
//
// Both paths share isCompileTimeStringConstant as the gate.
func runNoRbacDynamicClause(pass *analysis.Pass) (interface{}, error) {
	if pass.Pkg == nil {
		return nil, nil //nolint:nilnil // analyzer convention
	}
	// The rbac package's own tests call Where/For with literals, but
	// they also exercise internal helpers. Skip the package itself so
	// the analyzer never analyses its own subject — false positives
	// would be confusing and the package's own correctness is governed
	// by the doc-hardening pass (rbac.go SECURITY blocks on Where and
	// For).
	if pass.Pkg.Path() == rbacPackagePath {
		return nil, nil //nolint:nilnil
	}

	for _, file := range pass.Files {
		ast.Inspect(file, func(n ast.Node) bool {
			call, ok := n.(*ast.CallExpr)
			if !ok {
				return true
			}

			// Detection path A: method call on
			// (*rbac.ScopedQuery[T]).Where. The Fun is a bare
			// SelectorExpr (no IndexExpr wrapper — Where is not
			// generic).
			if sel, ok := call.Fun.(*ast.SelectorExpr); ok {
				if sel.Sel != nil && sel.Sel.Name == rbacWhereMethod &&
					isRbacScopedQueryReceiver(pass.TypesInfo, sel) {
					if len(call.Args) == 0 {
						// Wrong arity — go/types complains
						// elsewhere; this analyzer is only concerned
						// with the safety contract.
						return true
					}
					first := call.Args[0]
					if !isCompileTimeStringConstant(pass.TypesInfo, first) {
						pass.ReportRangef(first, "rbac.Where: first argument must be a Go string literal or untyped string constant; runtime values MUST flow through args... — see SPEC §10.1 / unblock-tv8.33")
					}
					return true
				}

				// Detection path C: method call on
				// (*rbac.ScopedQuery[T]).Columns. Columns is variadic
				// (cols ...string) and has NO bind channel — EVERY
				// argument is a SQL identifier sink, so every variadic
				// argument MUST be a compile-time string constant. Like
				// Where, the Fun is a bare SelectorExpr (Columns is not
				// generic) and the receiver resolves through Selections.
				if sel.Sel != nil && sel.Sel.Name == rbacColumnsMethod &&
					isRbacScopedQueryReceiver(pass.TypesInfo, sel) {
					for _, arg := range call.Args {
						if !isCompileTimeStringConstant(pass.TypesInfo, arg) {
							pass.ReportRangef(arg, "rbac.Columns: every column argument must be a Go string literal or untyped string constant; a runtime column identifier breaches the SELECT projection — see SPEC §10.1/§11.3 / unblock-8xb.8")
						}
					}
					return true
				}
			}

			// Detection path B: package-level generic call to rbac.For.
			// AST shape: CallExpr{Fun: IndexExpr|IndexListExpr{X:
			// SelectorExpr{rbac.For}}, Args: [identity, table]}. The
			// SelectorExpr is wrapped in the type-argument node and
			// must be unwrapped first.
			if sel := unwrapForSelector(call.Fun); sel != nil {
				if sel.Sel != nil && sel.Sel.Name == rbacForFunc &&
					isRbacForFunc(pass.TypesInfo, sel) {
					// For has signature For[T](identity, table) — table
					// is the SECOND positional arg. Require len > 1.
					if len(call.Args) < 2 {
						// Wrong arity — a call with fewer than two
						// arguments cannot type-check, so go/types
						// complains elsewhere; this analyzer is only
						// concerned with the safety contract (the table
						// argument being a compile-time constant) and has
						// nothing to assert when the table arg is absent.
						return true
					}
					second := call.Args[1]
					if !isCompileTimeStringConstant(pass.TypesInfo, second) {
						pass.ReportRangef(second, "rbac.For: second argument (table) must be a Go string literal or untyped string constant; runtime values would breach tenant isolation by rewriting the FROM clause and scope predicate — see SPEC §10.1 / unblock-tv8.35")
					}
					return true
				}
			}

			return true
		})
	}
	return nil, nil //nolint:nilnil
}

// unwrapForSelector returns the underlying *ast.SelectorExpr for a
// (potentially generic) call's Fun expression, or nil when the shape
// does not match.
//
// Three accepted shapes:
//
//   - *ast.SelectorExpr — non-generic call (kept for symmetry; rbac.For
//     is generic so this branch never hits in production but is cheap
//     and keeps the helper composable).
//   - *ast.IndexExpr{X: *ast.SelectorExpr} — single type-argument
//     instantiation, e.g. `rbac.For[Item](id, "items")`.
//   - *ast.IndexListExpr{X: *ast.SelectorExpr} — multi type-argument
//     instantiation (Go 1.18+), kept for forward compatibility.
//
// Anything else returns nil so the caller short-circuits the For
// detection branch cleanly.
func unwrapForSelector(fun ast.Expr) *ast.SelectorExpr {
	switch f := fun.(type) {
	case *ast.SelectorExpr:
		return f
	case *ast.IndexExpr:
		if sel, ok := f.X.(*ast.SelectorExpr); ok {
			return sel
		}
	case *ast.IndexListExpr:
		if sel, ok := f.X.(*ast.SelectorExpr); ok {
			return sel
		}
	}
	return nil
}

// isRbacForFunc returns true when sel resolves to the package-level
// generic function encore.app/shared/rbac.For. Resolution uses
// pass.TypesInfo.Uses (equivalently ObjectOf) on the selector ident:
// Selections is the wrong map for package-qualified identifiers — it
// records typed-receiver selectors, while a package-level func's
// SelectorExpr resolves through Uses. Mismatch silently fails:
// Selections.Lookup on a package-qualified ident returns no entry, so
// using Selections here would make the rule a no-op for For.
func isRbacForFunc(info *types.Info, sel *ast.SelectorExpr) bool {
	if info == nil || sel == nil || sel.Sel == nil {
		return false
	}
	obj := info.ObjectOf(sel.Sel)
	if obj == nil {
		return false
	}
	fn, ok := obj.(*types.Func)
	if !ok {
		return false
	}
	if fn.Pkg() == nil {
		return false
	}
	return fn.Pkg().Path() == rbacPackagePath && fn.Name() == rbacForFunc
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
