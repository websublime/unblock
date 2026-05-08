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

	"encore.app/auth"
)

// fakeIdentity returns a deterministic identity for builder tests.
func fakeIdentity(orgID string) auth.Identity {
	return auth.Identity{
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
