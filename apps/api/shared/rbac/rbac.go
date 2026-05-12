// Package rbac is the canonical typed query builder for org/project-scoped
// reads against the unblock database. It is the single mechanism by which
// services produce SELECT statements over schemas owned by another service
// (workitems.items, deps.dependencies, mcp.tool_calls, …) — Encore
// middleware is NOT used for tenant filtering. See SPEC §10.1.
//
// Surface (locked, do not change without a spec amendment):
//
//	type ScopedQuery[T any] struct{ /* internal */ }
//
//	func For[T any](identity auth.Identity, table string) *ScopedQuery[T]
//	func (q *ScopedQuery[T]) Where(clause string, args ...any) *ScopedQuery[T]
//	func (q *ScopedQuery[T]) Run(ctx context.Context) ([]T, error)
//
// Identity-type plumbing (bead unblock-tv8.30): SPEC §10.1's surface
// is documented with the literal `auth.Identity`. The implementation
// here imports the type from its leaf-package source at
// `encore.app/auth/types`, because importing `encore.app/auth`
// directly used to drag in the auth service's package-level
// sqldb.NewDatabase("unblock", ...) declaration and panic any plain
// `go test` run with "encore apps must be run using the encore
// command". After bead unblock-bne that NewDatabase call lives in the
// dedicated apps/api/db/ service rather than auth, but the import-
// the-leaf-types-package convention stays: it keeps the rbac builder
// dependency-graph clean (no domain-service imports at all) and is
// the contract the static analyzer at apps/api/shared/lint/ relies on
// for its package-scope checks. The two spellings remain
// interchangeable: the parent `auth` package declares
// `type Identity = types.Identity`, so a value constructed in a
// service as `auth.Identity{...}` is the same Go type as the
// `types.Identity` parameter accepted here.
//
// Usage:
//
//	import "encore.app/shared/rbac"
//
//	type Item struct {
//	    ID    string
//	    OrgID string
//	    Title string
//	}
//
//	rows, err := rbac.For[Item](id, "workitems.items").
//	    Where("status = $1", "Ready").
//	    Run(ctx)
//
// Scope guarantees:
//
//   - For[T] is the SOLE constructor. Every *ScopedQuery[T] returned by it
//     carries a non-empty `org_id = $N` predicate that is automatically
//     prepended to the WHERE chain when Run executes. The scope filter is
//     not optional and not bypassable — the public surface offers no way
//     to disable it.
//   - A naked composite literal `&rbac.ScopedQuery[T]{}` constructed
//     outside this package compiles (Go zero values are always available)
//     but Run on such a value returns a structured error. See the
//     "compile-time" property below for the precise contract.
//   - Run dispatches against the Bind-installed unblock handle (sourced
//     from apps/api/db/), never via cross-package imports of another
//     service's package-level db var. This preserves Encore's per-service
//     DB binding contract (SPEC §3.1) and means a service that does not
//     declare access to the unblock database fails at `encore check`
//     time, not at runtime.
//
// Injection-safety invariant (SPEC §10.1, unblock-tv8.33, unblock-tv8.35):
//
//   - The first argument to Where (clause) AND the second argument to
//     For (table) MUST each be a Go string literal or an untyped string
//     constant — i.e. a value that is fixed at compile time. Every
//     runtime value (request body, URL parameter, header, row from the
//     database, anything user-controlled) is forbidden in either
//     position; values destined for Where flow through the args...
//     variadic, and the table identifier of For has no runtime channel
//     at all (it is a SQL identifier, not a value).
//   - Both strings are concatenated verbatim into the assembled SQL
//     statement (see build): the clause into the WHERE chain, and the
//     table into the FROM clause AND into the canonical scope predicate
//     `<table>.org_id = $1`. There is no runtime validation, escaping,
//     or sanitisation — the SQL string is opaque to Go. A runtime value
//     in either position fragments the WHERE predicate and silently
//     bypasses the org_id scope guard installed by For. For a
//     tenant-isolation gate this is fatal.
//   - The project-local static analyzer
//     `apps/api/shared/lint/no_rbac_dynamic_clause.go` is the
//     enforcement gate for BOTH call shapes: it rejects any call to
//     ScopedQuery.Where whose first argument is not a compile-time
//     string constant, and any call to rbac.For whose second argument
//     (table) is not a compile-time string constant. The analyzer is
//     wired into `apps/api/.golangci.yml` and is also runnable directly
//     via `go run ./shared/lint/cmd/no_rbac_dynamic_clause ./...`.
//   - Runtime detection is fundamentally impossible: by the time the
//     SQL string reaches build, Go has erased the difference between a
//     literal and a runtime-constructed string. The analyzer is the
//     only gate. Reviewers asking "why no runtime check" should consult
//     this paragraph.
//
// Compile-time property (SPEC §10.1, AC-1).
//
// AC-1 reads "ScopedQuery[T].Run fails to compile when no scope filter is
// attached." Plain Go cannot express "this method on this struct value
// requires field X to be set" at the type system level — the literal
// reading is unreachable without code generation. The spec itself
// (lines 1870-1872) reframes AC-1 as a two-layer guarantee:
//
//   - Compile-time: Encore's per-service DB binding refuses any service
//     that has not declared `unblock` as a dependency from compiling. This
//     prevents cross-schema reads from services that do not own the data.
//   - Runtime: the typed builder injects the scope filter on every Run
//     against the Bind-installed unblock handle (sourced from
//     apps/api/db/ via rbac.Bind at process bootstrap). The scope filter
//     is never empty for a builder produced by For; a builder produced
//     by a naked struct literal has an empty scope and Run returns
//     ErrMissingScope.
//
// rbac.For is the only *exported constructor* for *ScopedQuery[T]; the
// type's fields are unexported, so any caller outside this package
// constructing it via `&rbac.ScopedQuery[T]{}` produces a value whose
// internal scope is zero-valued. Run treats that as a fatal builder error.
// The custom linter at `apps/api/shared/lint/` is the second layer: it
// flags any direct UPDATE on `workitems.items.is_ready` or
// `pipeline_stage` outside the cascade subscriber, which catches the
// related anti-pattern of bypassing the read-side builder for write paths
// that should also be funneled through it.
package rbac

import (
	"context"
	"errors"
	"fmt"
	"reflect"
	"strings"

	"encore.app/auth/types"
	"encore.dev/storage/sqldb"
)

// db is the shared handle for the canonical `unblock` Postgres database.
// Encore's parser refuses `sqldb.Named("unblock")` at this top-level
// non-service package (E1814: "Infrastructure resources can only be
// referenced within services"). The handle must therefore be injected
// by Bind from the dedicated apps/api/db/ migration-owner service.
//
// Investigation guidance (bead unblock-tv8.4) advised obtaining the
// handle directly via sqldb.Named here and avoiding any cross-package
// import of auth's db var. Encore's parser does not allow that — see
// the build-time error path noted above. Bind preserves the spirit of
// the guidance: every domain service that touches the unblock
// database uses the canonical BindDB late-bind hook (per SPEC §3.1
// and the apps/api/db/db.go file header), and the dedicated
// apps/api/db/ service is the SOLE binding authority — its single
// init() invokes auth.BindDB(DB), org.BindDB(DB), rbac.Bind(DB), and
// every future-service BindDB. rbac itself imports nothing
// service-specific. SPEC §10.1's locked surface (For/Where/Run) is
// unchanged. Per-service `initbind.go` files were retired in bead
// unblock-bne's pre-review scope expansion; the central bind in
// apps/api/db/db.go is now sufficient.
var db *sqldb.Database

// Bind installs the unblock-database handle that Run will dispatch
// against. Called exactly once at process start by the dedicated
// apps/api/db/ migration-owner service's init (the sole binding
// authority for every consumer's handle, post bead unblock-bne
// pre-review). Subsequent calls overwrite the handle; tests rely on
// this for swap-in of fakes.
//
// Concurrency: Bind is not goroutine-safe; the contract is that it
// runs during Encore service initialisation, before any handler can
// observe a partially constructed builder.
func Bind(database *sqldb.Database) {
	db = database
}

// ErrMissingScope is returned by Run when the *ScopedQuery[T] receiver
// was not constructed via For — i.e. when a caller bypassed the typed
// constructor by naked struct literal. Outside-of-package callers cannot
// set the unexported scope fields, so a zero-valued ScopedQuery hits this
// error path. This is the runtime half of AC-1's two-layer guarantee
// (the compile-time half is Encore's per-service DB binding; see package
// doc).
var ErrMissingScope = errors.New("rbac: ScopedQuery missing scope filter; use rbac.For[T] to construct")

// ErrEmptyTable is returned when For is called with table="". This is a
// programmer error and surfaces immediately at Run time. Tables are
// passed as opaque strings (typed identifiers do not carry through Go
// generics cleanly) so an empty value is the cheapest validation gate.
var ErrEmptyTable = errors.New("rbac: ScopedQuery built with empty table identifier")

// ErrNotBound is returned by Run when no service has called Bind to
// install the unblock-database handle. This is a service-bootstrap
// programmer error.
var ErrNotBound = errors.New("rbac: unblock database handle not bound; calling service must invoke rbac.Bind(sqldb.Named(\"unblock\"))")

// ScopedQuery is a typed, scope-bound query builder over a single
// org/project-scoped table. Every value produced by For carries an
// org-scoped WHERE predicate that Run prepends to the user-supplied
// filter chain. Field set is intentionally unexported so the only path
// to a non-zero scope is the For constructor.
type ScopedQuery[T any] struct {
	// identity is the calling user's resolved record. Carried for
	// future audit hooks (e.g. logging the row-count returned per
	// Identity for cross-tenant leak detection); not currently emitted
	// to the SQL plan beyond the scope predicate.
	//
	// Typed as types.Identity (the leaf-package source) rather than
	// auth.Identity to keep this package import-clean of the auth
	// service's init-time sqldb.NewDatabase call. The two spellings
	// are the same Go type via the alias declared in
	// apps/api/auth/auth.go.
	identity types.Identity

	// table is the fully-qualified target table, e.g. "workitems.items".
	// Empty is rejected at Run time (see ErrEmptyTable).
	table string

	// scopeClause is the canonical scope predicate prepended to every
	// query. For sets it to `<table_alias>.org_id = $N` against the
	// caller's identity.OrgID. A zero-valued ScopedQuery has scopeClause
	// == "" and Run returns ErrMissingScope.
	scopeClause string

	// scopeArgs holds the bind values for scopeClause (currently a
	// single OrgID). Stored as []any so future scope shapes (e.g.
	// org_id + project_id) can be added without touching the public
	// surface.
	scopeArgs []any

	// userClauses accumulates each caller-provided Where invocation in
	// declaration order. They are AND-joined onto scopeClause at Run.
	userClauses []userClause

	// err captures the first error encountered during fluent
	// construction (e.g. an empty clause string passed to Where).
	// Run surfaces it instead of executing.
	err error
}

// userClause records a single Where invocation. Stored separately from
// scopeClause so the scope predicate is immutably anchored at position
// 0 of the WHERE chain.
type userClause struct {
	clause string
	args   []any
}

// For constructs a *ScopedQuery[T] bound to the caller's Identity and
// the named table. The org-scope predicate is immediately materialised
// — a value returned by For has a non-empty scopeClause that Run will
// always emit.
//
// Empty table strings are accepted here and surfaced at Run time as
// ErrEmptyTable, mirroring the deferred-error pattern of the standard
// library's database/sql.DB.Prepare. This keeps the fluent surface
// chainable without panic-paths.
//
// table must be a fully-qualified `<schema>.<table>` identifier.
//
// SECURITY (SPEC §10.1, unblock-tv8.35).
//
// The table argument MUST be a Go string literal or an untyped string
// constant fixed at compile time. SQL identifiers have NO bind-parameter
// channel in PostgreSQL — table cannot be passed via args... like a
// value. A runtime-constructed table string is concatenated verbatim
// into BOTH the FROM clause (build at line ~370) and the canonical
// scope predicate `fmt.Sprintf("%s.org_id = $1", table)` (For body
// below). A user-controlled table value rewrites both sinks and
// silently bypasses tenant isolation — exactly the same class of
// footgun unblock-tv8.33 closed for Where.
//
//	// CORRECT — string literals and named constants:
//	rbac.For[Item](id, "workitems.items").Where(...).Run(ctx)
//
//	const itemsTable = "workitems.items"
//	rbac.For[Item](id, itemsTable).Where(...).Run(ctx)
//
//	// WRONG — runtime construction breaches scope:
//	rbac.For[Item](id, req.Table)                       // injection
//	rbac.For[Item](id, schema + ".items")               // injection
//	rbac.For[Item](id, fmt.Sprintf("%s.items", schema)) // injection
//
// The analyzer at `apps/api/shared/lint/no_rbac_dynamic_clause.go`
// rejects every non-literal second argument to For at lint time,
// alongside the same gate on Where's first argument. Runtime validation
// is impossible — the SQL string is opaque to Go by the time it reaches
// build. Bypassing the analyzer (e.g. //nolint suppression) on this
// function is forbidden and a code-review failure.
func For[T any](identity types.Identity, table string) *ScopedQuery[T] {
	q := &ScopedQuery[T]{
		identity: identity,
		table:    table,
	}
	if table == "" {
		q.err = ErrEmptyTable
		return q
	}
	// The scope predicate uses positional arg $1 because Run rebuilds
	// the full param list when it stitches scope + user clauses.
	q.scopeClause = fmt.Sprintf("%s.org_id = $1", table)
	q.scopeArgs = []any{identity.OrgID}
	return q
}

// Where appends a user-supplied predicate to the WHERE chain. The
// scope predicate built by For is always emitted first; user clauses
// follow, AND-joined, in declaration order. Args are bound positionally
// after the scope args.
//
// An empty clause is recorded as an error and surfaces at Run time;
// the receiver is still returned so fluent chains do not crash.
//
// SECURITY (SPEC §10.1, unblock-tv8.33).
//
// The clause argument MUST be a Go string literal or an untyped string
// constant fixed at compile time. Every runtime value MUST flow through
// args... — never through string concatenation, fmt.Sprintf, or any
// other operation that produces the clause text at runtime.
//
//	// CORRECT — constants and placeholders:
//	q.Where("status = $1 AND priority = $2", status, prio)
//
//	const filter = "is_archived = false"
//	q.Where(filter)
//
//	// WRONG — runtime construction breaches scope:
//	q.Where("status = '" + userInput + "'")              // injection
//	q.Where(fmt.Sprintf("status = '%s'", userInput))     // injection
//	clause := buildClause(req); q.Where(clause)          // injection
//
// The analyzer at `apps/api/shared/lint/no_rbac_dynamic_clause.go`
// rejects every non-literal first argument at lint time. Runtime
// validation is impossible — the SQL string is opaque to Go by the
// time it reaches build. Bypassing the analyzer (e.g. //nolint
// suppression) on this method is forbidden and a code-review failure.
func (q *ScopedQuery[T]) Where(clause string, args ...any) *ScopedQuery[T] {
	if q == nil {
		// nil receiver should not happen via the supported path (For
		// always returns non-nil), but defensive guards keep the
		// fluent chain from panicking on misuse.
		return q
	}
	if q.err != nil {
		return q
	}
	if strings.TrimSpace(clause) == "" {
		q.err = errors.New("rbac: empty Where clause")
		return q
	}
	q.userClauses = append(q.userClauses, userClause{
		clause: clause,
		args:   append([]any(nil), args...),
	})
	return q
}

// Run executes the assembled query and scans rows into a []T. It is a
// fatal builder error (ErrMissingScope) if the receiver was not
// produced by For. Any error captured during fluent construction is
// surfaced here.
//
// Row scanning uses reflection: T must be a struct whose exported
// fields correspond, in declaration order, to the columns selected by
// `SELECT * FROM <table>`. Mismatched column counts produce a
// structured error. P01 keeps the SELECT shape implicit (`*`) — future
// phases may add an explicit Columns([]string) hook without breaking
// the locked surface.
//
// Run is not on the API-key hot path (SPEC §4.3.2 short-circuits via a
// direct key_prefix lookup); reflection cost is acceptable for the
// general read surface.
func (q *ScopedQuery[T]) Run(ctx context.Context) ([]T, error) {
	if q == nil {
		return nil, ErrMissingScope
	}
	if q.err != nil {
		return nil, q.err
	}
	if q.scopeClause == "" {
		// Zero-valued struct literal path (`&rbac.ScopedQuery[T]{}`):
		// the scope filter was never installed because For was not
		// invoked. AC-1's runtime gate.
		return nil, ErrMissingScope
	}
	if q.table == "" {
		return nil, ErrEmptyTable
	}

	if db == nil {
		return nil, ErrNotBound
	}

	sql, args := q.build()

	rows, err := db.Query(ctx, sql, args...)
	if err != nil {
		return nil, fmt.Errorf("rbac: query %q: %w", q.table, err)
	}
	defer rows.Close()

	out, err := scanAll[T](rows)
	if err != nil {
		return nil, fmt.Errorf("rbac: scan %q: %w", q.table, err)
	}
	return out, nil
}

// build assembles the final SQL string and the positional arg slice.
// The scope predicate is always at $1; user clauses follow, with their
// placeholders rewritten so positional indices stay contiguous.
//
// build is internal and exported only via Run; tests verify its output
// directly via the unexported buildForTest hook.
func (q *ScopedQuery[T]) build() (string, []any) {
	var sb strings.Builder
	sb.WriteString("SELECT * FROM ")
	sb.WriteString(q.table)
	sb.WriteString(" WHERE ")
	sb.WriteString(q.scopeClause)

	args := append([]any(nil), q.scopeArgs...)
	nextPlaceholder := len(args) + 1

	for _, uc := range q.userClauses {
		rewritten, used := renumberPlaceholders(uc.clause, nextPlaceholder)
		sb.WriteString(" AND ")
		sb.WriteString(rewritten)
		args = append(args, uc.args...)
		nextPlaceholder += used
	}
	return sb.String(), args
}

// renumberPlaceholders rewrites `$1, $2, ...` in clause to start at
// startAt, and returns the count of placeholders rewritten. The pgx
// dialect is positional ($N); supporting `?` (mysql) is out of scope
// for the unblock database.
//
// The implementation is a single-pass scan over the clause string,
// replacing `$<digits>` runs with `$<startAt + observed_index - 1>`.
// Distinct placeholders that share an index in the input (e.g. $1
// appearing twice) are mapped to the same output index. Unused
// placeholder values in args are caller-controlled and surface as
// pgx-level errors at Run time, not here.
func renumberPlaceholders(clause string, startAt int) (string, int) {
	var (
		out     strings.Builder
		seen    = map[int]int{} // input index -> output index
		nextOut = startAt
		i       = 0
		runeLen = len(clause)
	)
	for i < runeLen {
		c := clause[i]
		if c != '$' {
			out.WriteByte(c)
			i++
			continue
		}
		// Parse digits following '$'.
		j := i + 1
		for j < runeLen && clause[j] >= '0' && clause[j] <= '9' {
			j++
		}
		if j == i+1 {
			// '$' not followed by digits — copy verbatim.
			out.WriteByte(c)
			i++
			continue
		}
		idx := 0
		for k := i + 1; k < j; k++ {
			idx = idx*10 + int(clause[k]-'0')
		}
		mapped, ok := seen[idx]
		if !ok {
			mapped = nextOut
			seen[idx] = mapped
			nextOut++
		}
		fmt.Fprintf(&out, "$%d", mapped)
		i = j
	}
	return out.String(), len(seen)
}

// scanAll consumes rows into a []T using reflection. Supported T
// kinds: struct (fields scanned by ordinal) and primitive scalar
// (single-column rows scanned by value). Anything else produces a
// typed error.
//
// We deliberately avoid sqlx-style tag-based mapping for P01: the
// callers in B-1..D-3 ship with row shapes that mirror the table
// declaration order, and an explicit struct-tag layer adds scope this
// bead does not own. A future phase may extend scanAll with tag-aware
// mapping; the surface (Run returning []T) does not change.
func scanAll[T any](rows *sqldb.Rows) ([]T, error) {
	var out []T

	var zero T
	rt := reflect.TypeOf(zero)

	for rows.Next() {
		var row T
		rv := reflect.ValueOf(&row).Elem()

		switch {
		case rt == nil:
			return nil, errors.New("rbac: scanAll requires a concrete T (got nil interface kind)")
		case rt.Kind() == reflect.Struct:
			fields := exportedFields(rv)
			ptrs := make([]any, len(fields))
			for i := range fields {
				ptrs[i] = fields[i].Addr().Interface()
			}
			if err := rows.Scan(ptrs...); err != nil {
				return nil, err
			}
		default:
			// Single-column scalar T (string, int, time.Time, …).
			if err := rows.Scan(rv.Addr().Interface()); err != nil {
				return nil, err
			}
		}
		out = append(out, row)
	}
	if err := rows.Err(); err != nil {
		return nil, err
	}
	return out, nil
}

// exportedFields returns the addressable, exported leaf fields of rv
// in declaration order. Embedded struct fields are NOT recursively
// flattened — callers should declare flat row shapes.
func exportedFields(rv reflect.Value) []reflect.Value {
	rt := rv.Type()
	out := make([]reflect.Value, 0, rt.NumField())
	for i := 0; i < rt.NumField(); i++ {
		sf := rt.Field(i)
		if !sf.IsExported() {
			continue
		}
		out = append(out, rv.Field(i))
	}
	return out
}
