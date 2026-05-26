// main_test.go owns the TestMain shape: seed the §11.1.0 fixture
// once at startup, run the suite, tear down once at exit. Per-subtest
// cleanup is deliberately avoided where possible — most §11.1.2
// assertions are read-only on the seeded graph (auth, prime, ready)
// or scoped to items the assertion creates itself (concurrent claim
// uses itm_b; cascade kinds mutate new edges between itm_c/itm_d;
// state-machine invariants seed their own helper items).
//
// The package is `exitcriteriontest_test` (external test) so the
// `_ "encore.app/db"` blank-import fires the BindDB chain before
// TestMain runs. Mirrors workitems/integration_test.go and
// shared/rbactest/rbactest_test.go.
//
// Encore-runtime requirement: this package MUST run under
// `encore test ./apps/api/exitcriteriontest/...`. See doc.go's
// "Encore-runtime requirement" section.

package exitcriteriontest_test

import (
	"context"
	"fmt"
	"os"
	"testing"

	// Importing encore.app/db triggers its init() which calls
	// auth.BindDB(DB), org.BindDB(DB), workitems.BindDB(DB),
	// deps.BindDB(DB), mcp.BindDB(DB), and rbac.Bind(DB). Without
	// this import every consumer's *sqldb.Database pointer stays nil
	// and SeedFixture returns the nil-handle error before any subtest
	// fires. Same pattern as workitems/integration_test.go and
	// shared/rbactest/rbactest_test.go.
	encoredb "encore.app/db"
	"encore.app/exitcriteriontest"
)

// fixture is the global, one-per-process fixture installed by
// TestMain. Read by every test body via the package-level helper
// fx() below. After TestMain returns, the value is effectively
// read-only (the Items map is populated once and never mutated; the
// test bodies that need additional rows seed them locally per-test).
var fixture *exitcriteriontest.Fixture

// fx returns the global fixture, t.Fatal-ing if SeedFixture failed
// in TestMain. Centralised so test bodies can't accidentally
// dereference a nil pointer; the t.Helper() call surfaces the
// failure at the test body's line, not inside this helper.
func fx(t *testing.T) *exitcriteriontest.Fixture {
	t.Helper()
	if fixture == nil {
		t.Fatal("exitcriteriontest: fixture is nil — TestMain seed must have failed (check Encore secret APIKeyHMACSecret + run under `encore test`)")
	}
	return fixture
}

// TestMain seeds the §11.1.0 fixture once, runs the suite, tears
// down once. fatalIf panics on nil-t to surface seed failures via
// the TestMain non-test panic path (no test has started, so we
// can't t.Fatal).
func TestMain(m *testing.M) {
	ctx := context.Background()

	var err error
	fixture, err = exitcriteriontest.SeedFixture(ctx, encoredb.DB)
	if err != nil {
		// TestMain has no *testing.T. Panic with the wrapped error
		// so the runner reports a clean diagnostic on the exact
		// failure path (seed.go errors include FK/path context).
		panic(fmt.Sprintf("exitcriteriontest: SeedFixture failed: %v", err))
	}

	code := m.Run()

	// Best-effort teardown. Failures are printed via fmt.Printf
	// (seed.go::Teardown does the printing) but never abort the
	// process — the test verdict is what the runner cares about.
	fixture.Teardown(ctx, encoredb.DB)

	os.Exit(code)
}
