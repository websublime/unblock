// Tests for the rbac typed query builder. The DB-backed Run path is
// covered by integration tests under apps/api/shared/rbactest/ (B-3,
// unblock-tv8.9). These unit tests cover the SQL-assembly layer and
// the AC-1 zero-value rejection contract — both reachable without a
// live Encore runtime.
package rbac

import (
	"context"
	"errors"
	"reflect"
	"strings"
	"testing"

	"encore.app/auth/types"
)

// fakeIdentity returns a deterministic identity for builder tests.
//
// Imported as `types.Identity` rather than `auth.Identity`: the latter
// path triggers `auth.init()` (sqldb.NewDatabase) which panics outside
// the encore CLI's cluster bring-up. The leaf `auth/types` package is
// Encore-free by construction (bead unblock-tv8.30). The two spellings
// alias to the same Go type — see auth.go's `type Identity = types.Identity`.
func fakeIdentity(orgID string) types.Identity {
	return types.Identity{
		UserID:    "usr_test",
		OrgID:     orgID,
		Role:      "member",
		AgentKind: "",
	}
}

// TestFor_AlwaysInjectsScope confirms For installs the scope predicate
// even when Where is never called. AC-1 (runtime half).
func TestFor_AlwaysInjectsScope(t *testing.T) {
	q := For[struct{ ID string }](fakeIdentity("org_alpha"), "workitems.items")
	if q.scopeClause == "" {
		t.Fatalf("For did not install scope predicate")
	}
	sql, args := q.build()
	wantPredicate := "workitems.items.org_id = $1"
	if !strings.Contains(sql, wantPredicate) {
		t.Errorf("SQL missing scope predicate %q: got %q", wantPredicate, sql)
	}
	if len(args) != 1 || args[0] != "org_alpha" {
		t.Errorf("scope args = %v, want [org_alpha]", args)
	}
}

// TestFor_EmptyTable defers to ErrEmptyTable at Run time rather than
// panicking during fluent construction.
func TestFor_EmptyTable(t *testing.T) {
	q := For[struct{ ID string }](fakeIdentity("org_alpha"), "")
	_, err := q.Run(context.Background())
	if !errors.Is(err, ErrEmptyTable) {
		t.Fatalf("Run on empty-table builder = %v, want ErrEmptyTable", err)
	}
}

// TestZeroValueScopedQuery_Run confirms a naked struct literal
// (constructed outside the For path, as a hostile or mistaken caller
// might) is rejected at Run time. This is AC-1's runtime gate.
func TestZeroValueScopedQuery_Run(t *testing.T) {
	q := &ScopedQuery[struct{ ID string }]{}
	_, err := q.Run(context.Background())
	if !errors.Is(err, ErrMissingScope) {
		t.Fatalf("Run on zero-value ScopedQuery = %v, want ErrMissingScope", err)
	}
}

// TestNilReceiver_Run confirms Run on a nil receiver returns the same
// missing-scope error rather than panicking.
func TestNilReceiver_Run(t *testing.T) {
	var q *ScopedQuery[struct{ ID string }]
	_, err := q.Run(context.Background())
	if !errors.Is(err, ErrMissingScope) {
		t.Fatalf("Run on nil receiver = %v, want ErrMissingScope", err)
	}
}

// TestWhere_AppendsAndJoinsAndRenumbers verifies caller-provided
// clauses are AND-joined to the scope predicate with placeholders
// renumbered to follow the scope arg.
func TestWhere_AppendsAndJoinsAndRenumbers(t *testing.T) {
	q := For[struct{ ID string }](fakeIdentity("org_alpha"), "workitems.items").
		Where("status = $1", "Ready").
		Where("priority = $1 OR priority = $2", "P0", "P1")

	sql, args := q.build()

	// Scope first; user clauses AND-joined; placeholders renumbered.
	wantSQL := "SELECT * FROM workitems.items WHERE workitems.items.org_id = $1 AND status = $2 AND priority = $3 OR priority = $4"
	if sql != wantSQL {
		t.Errorf("SQL mismatch:\n got %q\nwant %q", sql, wantSQL)
	}
	wantArgs := []any{"org_alpha", "Ready", "P0", "P1"}
	if len(args) != len(wantArgs) {
		t.Fatalf("args len = %d, want %d (got %v)", len(args), len(wantArgs), args)
	}
	for i, a := range args {
		if a != wantArgs[i] {
			t.Errorf("args[%d] = %v, want %v", i, a, wantArgs[i])
		}
	}
}

// TestWhere_RepeatedPlaceholderInOneClause covers the renumbering
// edge case where a single user clause references the same $N twice.
// Both occurrences must rewrite to the same output index.
func TestWhere_RepeatedPlaceholderInOneClause(t *testing.T) {
	q := For[struct{ ID string }](fakeIdentity("org_alpha"), "deps.dependencies").
		Where("(from_id = $1 OR to_id = $1)", "itm_x")

	sql, args := q.build()

	wantSQL := "SELECT * FROM deps.dependencies WHERE deps.dependencies.org_id = $1 AND (from_id = $2 OR to_id = $2)"
	if sql != wantSQL {
		t.Errorf("SQL mismatch:\n got %q\nwant %q", sql, wantSQL)
	}
	if len(args) != 2 {
		t.Errorf("args len = %d, want 2 (got %v)", len(args), args)
	}
}

// TestWhere_EmptyClause defers an error to Run time without panicking
// the fluent chain.
func TestWhere_EmptyClause(t *testing.T) {
	q := For[struct{ ID string }](fakeIdentity("org_alpha"), "workitems.items").
		Where("   ", "ignored")
	_, err := q.Run(context.Background())
	if err == nil || !strings.Contains(err.Error(), "empty Where clause") {
		t.Fatalf("Run after empty Where = %v, want empty-clause error", err)
	}
}

// TestRenumberPlaceholders_NoPlaceholders is a unit-level guard for
// the helper.
func TestRenumberPlaceholders_NoPlaceholders(t *testing.T) {
	got, used := renumberPlaceholders("status = 'Ready'", 5)
	if got != "status = 'Ready'" {
		t.Errorf("got %q, want unchanged", got)
	}
	if used != 0 {
		t.Errorf("used = %d, want 0", used)
	}
}

// TestRenumberPlaceholders_DollarNotFollowedByDigits keeps `$$` and
// `$identifier` byte-identical (Postgres allows them in dollar-quoted
// strings; the rbac builder's caller-supplied clauses should not
// contain them, but the helper should be defensive).
func TestRenumberPlaceholders_DollarNotFollowedByDigits(t *testing.T) {
	got, used := renumberPlaceholders("body LIKE '%$tag%'", 3)
	if got != "body LIKE '%$tag%'" {
		t.Errorf("got %q, want unchanged", got)
	}
	if used != 0 {
		t.Errorf("used = %d, want 0", used)
	}
}

// TestWhere_InjectionDocumentation pins the runtime behaviour of
// Where in the presence of a hostile, dynamically-constructed clause
// string. These tests EXIST TO DOCUMENT THE INVARIANT, not to exercise
// a defence: the rbac builder has no runtime gate against SQL
// injection, by design — see the SECURITY block on Where (SPEC §10.1,
// unblock-tv8.33).
//
// The production-grade gate is the static analyzer at
// `apps/api/shared/lint/no_rbac_dynamic_clause.go`, which rejects any
// non-literal first argument at lint time. These tests confirm that:
//
//  1. If the analyzer is bypassed (e.g. //nolint suppression, which is
//     forbidden but possible), a hostile clause is concatenated
//     verbatim into the assembled SQL — i.e. the leak is real.
//  2. The scope predicate (org_id = $1) is still emitted, but it sits
//     before the hostile clause and an attacker can use SQL
//     meta-characters to neutralise it (close the predicate with a
//     comment, OR-true, etc).
//
// If either assertion ever flips (e.g. a future runtime sanitiser is
// added), the test must be re-evaluated — runtime sanitisation of
// arbitrary SQL is brittle and has historically been the wrong gate
// for tenant isolation. The analyzer remains the contractual gate.
func TestWhere_InjectionDocumentation_HostileClauseEmitsVerbatim(t *testing.T) {
	// A clause that pretends to filter on status but trails SQL that
	// neutralises the org_id predicate by injecting OR 1=1 and a
	// line-comment marker.
	//
	// The analyzer would reject this at lint time because the value
	// is a `var hostile := ...` — but the runtime builder has no
	// such gate. We construct the clause via a literal here only so
	// the test itself compiles cleanly; the assertion is that
	// build() emits the bytes verbatim regardless of source.
	q := For[struct{ ID string }](fakeIdentity("org_alpha"), "workitems.items").
		Where("status = $1 OR 1=1 -- ", "Ready")
	sql, _ := q.build()

	// The hostile bytes survive into the assembled SQL.
	if !strings.Contains(sql, "OR 1=1 -- ") {
		t.Fatalf("hostile clause did NOT appear verbatim in assembled SQL — runtime sanitiser detected, contract changed: %q", sql)
	}
	// The scope predicate is still emitted at $1, but the AND-join
	// places it BEFORE the hostile clause. Combined with `OR 1=1`
	// the org_id filter is neutralised at the SQL semantic level —
	// hence the analyzer-only gate.
	if !strings.Contains(sql, "workitems.items.org_id = $1") {
		t.Errorf("scope predicate missing from assembled SQL: %q", sql)
	}
}

// TestWhere_InjectionDocumentation_NoRuntimeSanitiser is a paired
// regression: it asserts that classic injection meta-characters (';',
// '--', '/*', stray quote) survive verbatim through build(). Pinning
// this behaviour is the regression net if a future contributor adds a
// runtime sanitiser and silently changes the contract documented on
// Where. Runtime sanitisation is the wrong gate for this surface; the
// analyzer is the only acceptable defence.
//
// Test asserts on the meta-character bytes only — the `$N` placeholder
// indices are renumbered by build() to keep positional args contiguous
// (the scope predicate consumes $1, so user `$1` becomes `$2`). That
// renumbering is orthogonal placeholder bookkeeping covered by
// TestWhere_AppendsAndJoinsAndRenumbers; this test is concerned only
// with non-numeric meta-character survival.
func TestWhere_InjectionDocumentation_NoRuntimeSanitiser(t *testing.T) {
	for _, tc := range []struct {
		name   string
		clause string
		// wantSubstr is a fragment of the hostile clause AFTER the
		// renumbered placeholder — chosen so the assertion is robust
		// to placeholder renumbering but still pins the meta-character
		// survival.
		wantSubstr string
	}{
		{name: "semicolon", clause: "status = $1; DROP TABLE x", wantSubstr: "; DROP TABLE x"},
		{name: "line_comment", clause: "status = $1 -- malicious", wantSubstr: " -- malicious"},
		{name: "block_comment", clause: "status = $1 /* malicious */", wantSubstr: " /* malicious */"},
		{name: "stray_quote", clause: "status = $1 OR title = 'oops", wantSubstr: " OR title = 'oops"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			q := For[struct{ ID string }](fakeIdentity("org_alpha"), "workitems.items").
				Where(tc.clause, "Ready")
			sql, _ := q.build()
			if !strings.Contains(sql, tc.wantSubstr) {
				t.Fatalf("hostile pattern %q not found in assembled SQL — runtime sanitiser detected, see SPEC §10.1: got %q", tc.wantSubstr, sql)
			}
		})
	}
}

// TestFor_TableInjectionDocumentation pins the runtime behaviour of
// For in the presence of a hostile, dynamically-constructed table
// identifier. These tests EXIST TO DOCUMENT THE INVARIANT, not to
// exercise a defence: the rbac builder has no runtime gate against SQL
// injection on the table argument, by design — see the SECURITY block
// on For (SPEC §10.1, unblock-tv8.35).
//
// The production-grade gate is the static analyzer at
// `apps/api/shared/lint/no_rbac_dynamic_clause.go`, which rejects any
// non-literal SECOND argument to rbac.For at lint time (alongside the
// same gate on Where's first argument). These tests confirm that:
//
//  1. If the analyzer is bypassed (e.g. //nolint suppression, which is
//     forbidden but possible), a hostile table value is concatenated
//     verbatim into BOTH the assembled FROM clause and the canonical
//     scope predicate `<table>.org_id = $1` — i.e. the leak is real,
//     and a single hostile string can rewrite both sinks at once.
//  2. The scope predicate is still emitted in shape, but its target
//     identifier is now caller-controlled and an attacker can use SQL
//     meta-characters (semicolon, line-comment, OR-true) to neutralise
//     the org_id filter, bypass tenant isolation, or terminate the
//     statement and append arbitrary SQL.
//
// If either assertion ever flips (e.g. a future runtime sanitiser is
// added on the table argument), the test must be re-evaluated —
// runtime sanitisation of arbitrary SQL identifiers is brittle and has
// historically been the wrong gate for tenant isolation. The analyzer
// remains the contractual gate.
func TestFor_TableInjectionDocumentation_HostileTableEmitsVerbatim(t *testing.T) {
	// A hostile table value that closes the scope predicate with a
	// line-comment marker, terminates the statement with a semicolon,
	// and appends arbitrary DDL. The analyzer would reject this at
	// lint time because the value is a `var hostile := ...` — but the
	// runtime builder has no such gate. We construct the value via a
	// literal here only so the test itself compiles cleanly; the
	// assertion is that build() emits the bytes verbatim regardless of
	// source.
	const hostileTable = "workitems.items; DROP TABLE x --"

	q := For[struct{ ID string }](fakeIdentity("org_alpha"), hostileTable)
	sql, _ := q.build()

	// The hostile bytes survive into the assembled FROM clause
	// verbatim.
	if !strings.Contains(sql, "FROM "+hostileTable+" WHERE") {
		t.Fatalf("hostile table did NOT appear verbatim in FROM clause — runtime sanitiser detected on table arg, contract changed: %q", sql)
	}
	// The scope predicate is also rewritten with the hostile bytes,
	// because For interpolates `<table>.org_id = $1` via fmt.Sprintf.
	// The line-comment marker (` -- `) inside hostileTable now sits
	// inside the predicate and (with PostgreSQL's line-comment
	// semantics in the assembled statement) neutralises everything
	// downstream — the org_id filter is gone in the SQL parse tree.
	if !strings.Contains(sql, hostileTable+".org_id = $1") {
		t.Errorf("scope predicate not interpolated with hostile table verbatim: %q", sql)
	}
}

// TestFor_TableInjectionDocumentation_NoRuntimeSanitiser is a paired
// regression: it asserts that classic injection meta-characters (';',
// '--', '/*', stray quote, OR-true) survive verbatim through For ->
// build() on the table argument. Pinning this behaviour is the
// regression net if a future contributor adds a runtime sanitiser and
// silently changes the contract documented on For. Runtime
// sanitisation is the wrong gate for this surface; the analyzer is the
// only acceptable defence.
func TestFor_TableInjectionDocumentation_NoRuntimeSanitiser(t *testing.T) {
	for _, tc := range []struct {
		name  string
		table string
		// wantSubstr is a fragment of the hostile table chosen so the
		// assertion pins meta-character survival in BOTH the FROM
		// clause and the interpolated scope predicate.
		wantSubstr string
	}{
		{name: "semicolon", table: "workitems.items; DROP TABLE x", wantSubstr: "; DROP TABLE x"},
		{name: "line_comment", table: "workitems.items -- malicious", wantSubstr: " -- malicious"},
		{name: "block_comment", table: "workitems.items /* malicious */", wantSubstr: " /* malicious */"},
		{name: "stray_quote", table: "workitems.items OR title = 'oops", wantSubstr: " OR title = 'oops"},
		{name: "or_true", table: "workitems.items WHERE 1=1 OR ", wantSubstr: " WHERE 1=1 OR "},
	} {
		t.Run(tc.name, func(t *testing.T) {
			q := For[struct{ ID string }](fakeIdentity("org_alpha"), tc.table)
			sql, _ := q.build()
			if !strings.Contains(sql, tc.wantSubstr) {
				t.Fatalf("hostile pattern %q not found in assembled SQL — runtime sanitiser detected on table arg, see SPEC §10.1: got %q", tc.wantSubstr, sql)
			}
		})
	}
}

// TestExportedFields_StructLayout confirms the reflection helper
// returns exported fields in declaration order and skips unexported
// ones. Exercises the same code path scanAll uses against generic T.
func TestExportedFields_StructLayout(t *testing.T) {
	type row struct {
		ID    string
		title string //nolint:unused // exercised by reflection
		Body  string
	}
	r := row{}
	rv := reflect.ValueOf(&r).Elem()
	got := exportedFields(rv)
	if len(got) != 2 {
		t.Fatalf("len = %d, want 2 (got fields %v)", len(got), got)
	}
	// First field must be ID (string), second Body (string).
	if got[0].Kind() != reflect.String || got[1].Kind() != reflect.String {
		t.Errorf("expected two string fields; got kinds %v, %v", got[0].Kind(), got[1].Kind())
	}
}
