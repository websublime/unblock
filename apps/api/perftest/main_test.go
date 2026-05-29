// main_test.go owns the TestMain shape: the UNBLOCK_PERF_GATE
// short-circuit gate, then (when gated on) seed the perftest fixture
// once at startup, run the suite, tear down once at exit.
//
// The package is `perftest_test` (external test) so the
// `_ "encore.app/db"` blank-import fires the BindDB chain before
// TestMain runs. Mirrors exitcriteriontest/main_test.go.
//
// Encore-runtime requirement: this package MUST run under
// `encore test ./apps/api/perftest/...`. See doc.go.
//
// Test isolation (SPEC §11.2 NFR-1, round-15). The harness is excluded
// from the default `encore test ./...` suite (Gate 5) by a TestMain
// short-circuit keyed on UNBLOCK_PERF_GATE. See the gate block in
// TestMain below and the "Test isolation" section of doc.go for the
// full rationale (CI run 26633703926).

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

// TestMain is the perftest harness's isolation gate AND lifecycle owner.
//
// Test isolation (SPEC §11.2 NFR-1, round-15). When UNBLOCK_PERF_GATE is
// NOT "1" — which is the case for the default `encore test ./...` suite
// (Gate 5) — TestMain returns IMMEDIATELY with exit 0 WITHOUT calling
// perftest.SeedFixture and WITHOUT calling m.Run(). The package therefore
// contributes ZERO database load, ZERO mcp.tool_calls rows, and runs
// neither its measurement loop nor its negative-auth loop in the default
// suite — satisfying the round-15 default-suite contract ("no seed, no
// loops, not merely log-and-pass") at the earliest possible point. This
// is a run-time short-circuit rather than a `//go:build perf` compile-time
// exclusion because Encore's test codegen does not propagate custom build
// tags to its generated runtime shims (rlog, secrets): `encore test
// -tags=perf` fails to build the whole app (undefined rlog.Error,
// __encore_secrets.Load). The env-var gate keeps the package always
// compiling — so the Encore parser and the default suite stay green — while
// still guaranteeing zero side effects when the gate is off.
//
// Empirical justification (CI run 26633703926): under the shared local
// Postgres the harness's ~630 concurrent prime/ready/claim MCP calls
// ballooned warm-cache latency from ~87 ms to 5–16 s, tripped a non-gated
// "no SSE data" t.Fatalf, and broke a sibling package's tool_calls
// assertion. The harness fundamentally cannot co-schedule with the full
// functional suite on shared infra.
//
// When gated on (UNBLOCK_PERF_GATE=1, the dedicated isolated CI step owned
// by Olive), TestMain seeds the fixture once, runs the suite, and tears
// down once.
func TestMain(m *testing.M) {
	// Isolation gate: the default suite (gate unset) does nothing at all.
	if os.Getenv(perftest.PerfGateEnv) != "1" {
		os.Exit(0)
	}

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
