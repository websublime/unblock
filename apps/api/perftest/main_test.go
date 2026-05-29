// main_test.go owns the TestMain shape: seed the perftest fixture once
// at startup, run the suite, tear down once at exit.
//
// The package is `perftest_test` (external test) so the
// `_ "encore.app/db"` blank-import fires the BindDB chain before
// TestMain runs. Mirrors exitcriteriontest/main_test.go.
//
// Encore-runtime requirement: this package MUST run under
// `encore test ./apps/api/perftest/...`. See doc.go.

package perftest_test

import (
	"context"
	"fmt"
	"os"
	"testing"

	// Importing encore.app/db triggers its init() which calls
	// auth.BindDB(DB), org.BindDB(DB), workitems.BindDB(DB),
	// deps.BindDB(DB), mcp.BindDB(DB), and rbac.Bind(DB). Without this
	// import every consumer's *sqldb.Database pointer stays nil and
	// SeedFixture returns the nil-handle error before any subtest
	// fires.
	encoredb "encore.app/db"
	"encore.app/perftest"
)

// fixture is the global, one-per-process fixture installed by
// TestMain. Read by every test body via fx().
var fixture *perftest.Fixture

// fx returns the global fixture, t.Fatal-ing if SeedFixture failed in
// TestMain.
func fx(t *testing.T) *perftest.Fixture {
	t.Helper()
	if fixture == nil {
		t.Fatal("perftest: fixture is nil — TestMain seed must have failed (check Encore secret APIKeyHMACSecret + run under `encore test`)")
	}
	return fixture
}

// TestMain seeds the perftest fixture once, runs the suite, tears down
// once.
func TestMain(m *testing.M) {
	ctx := context.Background()

	var err error
	fixture, err = perftest.SeedFixture(ctx, encoredb.DB)
	if err != nil {
		// TestMain has no *testing.T. Panic with the wrapped error so
		// the runner reports a clean diagnostic on the exact failure
		// path.
		panic(fmt.Sprintf("perftest: SeedFixture failed: %v", err))
	}

	code := m.Run()

	// Best-effort teardown; failures are logged via rlog inside
	// Teardown but never abort the process.
	fixture.Teardown(ctx, encoredb.DB)

	os.Exit(code)
}
