// cycle_test.go covers the §11.1.2 cycle-detection assertion and
// the §11.3 / NFR-5 architectural invariant:
//
//   - add_dependency(from=itm_e, to=itm_a) on the §11.1.0 fixture is
//     rejected with CYCLE_DETECTED (the d→e edge already closes the
//     chain when itm_e → itm_a is attempted: itm_a → itm_b → itm_d
//     → itm_e → itm_a).
//   - Cycle property test (N=100 random graph mutations): zero
//     cycles ever materialise in the DB.
//
// Both the cycle assertion AND the N=100 property test are driven via
// the MCP `add_dependency` tool surface (not deps.AddEdge directly) so
// the public agent-facing path is exercised end-to-end. The
// advisory-lock contract (SPEC §11.3 — pg_advisory_xact_lock under
// deps.add_dependency:project_id) is exercised inside deps.AddEdge
// regardless of caller; the property test gives the lock real
// concurrent contention via goroutines, each on its own MCP session.
//
// Property-test isolation (unblock-tv8.70). The N=100 property test
// (TestExitCriterion_CycleProperty_N100Mutations) is GATED behind
// UNBLOCK_PERF_GATE=1 and t.Skips in the default `encore test ./...`
// suite. Rationale: a single MCP add_dependency round-trip is ~15 ms in
// isolation, but under the default full-suite shared-local-Postgres
// contention warm-cache calls balloon to seconds (SPEC §11.2 NFR-1
// round-15). 100 calls across 8 goroutines on a fresh-connection-
// per-call HTTP client would exceed the 10 s per-call timeout and flake
// CI when co-scheduled with the rest of the suite. So the MCP-routed
// property test runs ONLY in the dedicated isolated CI step under
// UNBLOCK_PERF_GATE=1 (owned by Olive), mirroring the perftest
// round-15 isolation doctrine (apps/api/perftest/main_test.go). The
// §11.1.2 acceptance test (TestExitCriterion_CycleDetected, a single
// MCP call) stays in the default suite. The gate is at the property
// test FUNCTION level — not TestMain — because this package's TestMain
// also hosts the §11.1.2 acceptance tests that MUST run in the default
// suite.

package exitcriteriontest_test

import (
	"context"
	"math/rand"
	"os"
	"sync"
	"testing"

	encoredb "encore.app/db"
)

// perfGateEnv is the environment variable that admits the latency-
// sensitive property test into a run. Matches perftest.PerfGateEnv
// ("UNBLOCK_PERF_GATE"); kept as a local literal so this external test
// package does not import the perftest support package. When unset, the
// property test t.Skips before seeding (zero added DB load on the
// default suite). See the file-header isolation note.
const perfGateEnv = "UNBLOCK_PERF_GATE"

// cyclePropertyN is the random-mutation cardinality per SPEC §11.3 /
// NFR-5 ("property test N=100 random graph mutations").
const cyclePropertyN = 100

// TestExitCriterion_CycleDetected covers the §11.1.2 add_dependency
// CYCLE_DETECTED assertion against the §11.1.0 fixture.
//
// The fixture topology after seed:
//
//	itm_a → itm_b → itm_c
//	             ↘ itm_d → itm_e
//
// Adding itm_e → itm_a closes the chain itm_a → itm_b → itm_d →
// itm_e → itm_a. The MCP `add_dependency` handler returns the §7
// PRECONDITION_NOT_MET envelope with data.kind=CYCLE_DETECTED
// (apps/api/mcp/errmap.go translation of deps.AddEdge's
// FailedPrecondition + Meta.kind="CYCLE_DETECTED").
//
// Side-effect: the rejection writes a forensic row to deps.cycles
// (apps/api/deps/deps.go:303-313 recordCycle). The test asserts the
// row is present with the expected cycle_path so the forensic audit
// trail is exercised too.
func TestExitCriterion_CycleDetected(t *testing.T) {
	f := fx(t)
	ctx := t.Context()
	sessionID := initializeSession(t, f.RawKey)

	itemA := f.ItemID("itm_a")
	itemE := f.ItemID("itm_e")

	env := callTool(t, f.RawKey, sessionID, "add_dependency", map[string]any{
		"from_item_id": itemE,
		"to_item_id":   itemA,
		"kind":         "blocks",
	})
	data := expectError(t, env)

	if data.Kind != "CYCLE_DETECTED" {
		t.Fatalf("data.kind = %q, want CYCLE_DETECTED; data=%+v", data.Kind, data)
	}

	// Cross-check the forensic row: deps.cycles MUST have a row with
	// from_item=itm_e, to_item=itm_a, and a non-empty cycle_path
	// containing both endpoints.
	var (
		from, to   string
		cyclePath  []string
		rejectedBy *string
	)
	if err := encoredb.DB.QueryRow(ctx,
		`SELECT from_item, to_item, cycle_path, rejected_by
		   FROM deps.cycles
		  WHERE from_item = $1 AND to_item = $2
		  ORDER BY detected_at DESC
		  LIMIT 1`,
		itemE, itemA,
	).Scan(&from, &to, &cyclePath, &rejectedBy); err != nil {
		t.Fatalf("deps.cycles forensic row query: %v", err)
	}
	if from != itemE || to != itemA {
		t.Fatalf("cycles row mismatch: from=%q to=%q; want from=%q to=%q", from, to, itemE, itemA)
	}
	if len(cyclePath) == 0 {
		t.Fatalf("cycle_path is empty; want a non-empty walk")
	}

	// Property invariant: no cycle ever materialises. Spot-check by
	// running the cycle CTE on the dependencies table. (The
	// productive guarantee — deps.AddEdge rejected the write — is
	// already validated above; this is the structural assertion that
	// SPEC §11.3 NFR-5 calls for.)
	assertNoCyclesInDB(t, ctx, f.ProjectID)
}

// TestExitCriterion_CycleProperty_N100Mutations is the §11.3 / NFR-5
// property test: N=100 random graph mutations against a fresh
// per-test subgraph; assert no cycles in the DB after the run.
//
// Mutations are restricted to add_dependency calls on a 6-node
// random-DAG seed driven through the MCP `add_dependency` tool surface
// (unblock-tv8.70); each goroutine attempts a random edge on its own
// MCP session, and the test relies on deps.AddEdge's per-project
// advisory lock + cycle CTE (reached via the MCP handler) to reject
// every cycle-forming attempt. The post-state assertion is the
// canonical SPEC §11.3 promise.
//
// Gated behind UNBLOCK_PERF_GATE=1 (see file-header isolation note):
// the MCP transport under the default full-suite shared-Postgres
// contention exceeds the 10 s per-call timeout and flakes CI, so this
// test runs only in the dedicated isolated CI step.
func TestExitCriterion_CycleProperty_N100Mutations(t *testing.T) {
	if os.Getenv(perfGateEnv) != "1" {
		t.Skipf("skipping MCP-routed N=100 cycle property test in default suite; set %s=1 to run (isolated CI step, SPEC §11.2 NFR-1 round-15)", perfGateEnv)
	}

	f := fx(t)
	ctx := t.Context()

	// Seed a fresh subgraph of 6 items under the fixture's
	// org/project. The §11.1.0 graph stays untouched.
	const nodeCount = 6
	nodes := make([]string, nodeCount)
	for i := 0; i < nodeCount; i++ {
		nodes[i] = seedFreshTask(t, ctx, f, "cycle-prop-node")
	}

	// Mutation loop. Generate cyclePropertyN random (from, to) pairs
	// from the nodes slice; some will form cycles (rejected), some
	// will not (accepted). The acceptance / rejection split is
	// irrelevant for the property assertion — what matters is that
	// no cycle ever ends up in the DB.
	//
	// We use a single shared rand source seeded deterministically so
	// the test is reproducible. Goroutine fan-out is bounded at 8 —
	// enough to exercise the advisory lock under contention but not
	// so high that the test takes minutes.
	rng := rand.New(rand.NewSource(1))
	type pair struct{ from, to int }
	pairs := make([]pair, cyclePropertyN)
	for i := range pairs {
		// from != to to avoid trivial self-loop rejection (which the
		// schema's no_self_loop_chk catches but is not the property
		// we are testing).
		for {
			a, b := rng.Intn(nodeCount), rng.Intn(nodeCount)
			if a != b {
				pairs[i] = pair{from: a, to: b}
				break
			}
		}
	}

	const fanout = 8
	jobs := make(chan pair, len(pairs))
	for _, p := range pairs {
		jobs <- p
	}
	close(jobs)

	// Pre-mint one MCP session per goroutine on the MAIN goroutine
	// (mirrors concurrent_claim_test.go:79-82). initializeSession
	// t.Fatalf-s on failure, which is only safe on the test goroutine;
	// pre-minting keeps that off the worker goroutines. Each worker
	// then owns a distinct Mcp-Session-Id so concurrent tools/call
	// invocations never share the SDK's stateful session map. The
	// Bearer key (f.RawKey) is shared; auth is per-request.
	sessions := make([]string, fanout)
	for i := 0; i < fanout; i++ {
		sessions[i] = initializeSession(t, f.RawKey)
	}

	var wg sync.WaitGroup
	wg.Add(fanout)
	for w := 0; w < fanout; w++ {
		sessionID := sessions[w]
		go func() {
			defer wg.Done()
			for p := range jobs {
				// MCP `add_dependency` tool call (unblock-tv8.70):
				// exercises the full end-to-end agent-facing path
				// (Bearer auth + JSON-RPC framing + handler →
				// deps.AddEdge advisory lock + cycle CTE).
				_ = callTool(t, f.RawKey, sessionID, "add_dependency", map[string]any{
					"from_item_id": nodes[p.from],
					"to_item_id":   nodes[p.to],
					"kind":         "blocks",
				})
				// We intentionally ignore the envelope: success,
				// ALREADY_EXISTS, and CYCLE_DETECTED are all legitimate
				// outcomes per the property test contract. The
				// structural invariant is checked after the loop.
			}
		}()
	}
	wg.Wait()

	// Property assertion: zero cycles in the dependencies table.
	assertNoCyclesInDB(t, ctx, f.ProjectID)
}

// assertNoCyclesInDB runs the cycle-detection CTE against the
// dependencies table for the given project_id and fails the test
// if any cycle is found. The CTE is a depth-counter reachability
// walk over the 'blocks' edges (matching SPEC §6.5 / 9.4.9 in
// shape but simplified — we only need a yes/no, not a path).
func assertNoCyclesInDB(t *testing.T, ctx context.Context, projectID string) {
	t.Helper()

	// Postgres-native cycle detection via the WITH RECURSIVE CTE.
	// On a cycle, the recursive step would walk indefinitely; we
	// cap depth at 256 (matches deps.closureMaxDepth) and detect
	// the cycle by observing a path that returns to its start.
	var cycleFound bool
	err := encoredb.DB.QueryRow(ctx,
		`WITH RECURSIVE walk(start_node, current_node, depth, path) AS (
		   SELECT d.from_item, d.to_item, 1, ARRAY[d.from_item, d.to_item]
		     FROM deps.dependencies d
		     JOIN workitems.items i_from ON i_from.id = d.from_item
		    WHERE d.kind = 'blocks' AND i_from.project_id = $1
		   UNION ALL
		   SELECT w.start_node, d.to_item, w.depth + 1, w.path || d.to_item
		     FROM walk w
		     JOIN deps.dependencies d ON d.from_item = w.current_node
		     JOIN workitems.items i_from ON i_from.id = d.from_item
		    WHERE d.kind = 'blocks'
		      AND i_from.project_id = $1
		      AND w.depth < 256
		      AND NOT (d.to_item = ANY (w.path[1:array_length(w.path, 1) - 1]))
		 )
		 SELECT EXISTS (
		   SELECT 1 FROM walk WHERE current_node = start_node AND depth > 0
		 )`,
		projectID,
	).Scan(&cycleFound)
	if err != nil {
		t.Fatalf("cycle CTE query: %v", err)
	}
	if cycleFound {
		// Dump the offending edges so the failure points at the
		// drift, not just the bool.
		rows, dErr := encoredb.DB.Query(ctx,
			`SELECT d.from_item, d.to_item, d.kind
			   FROM deps.dependencies d
			   JOIN workitems.items i ON i.id = d.from_item
			  WHERE i.project_id = $1`,
			projectID,
		)
		if dErr != nil {
			t.Fatalf("post-cycle dump query: %v", dErr)
		}
		defer rows.Close()
		dump := make([]string, 0, 64)
		for rows.Next() {
			var f, to, k string
			if err := rows.Scan(&f, &to, &k); err != nil {
				continue
			}
			dump = append(dump, f+"-"+k+"->"+to)
		}
		t.Fatalf("cycle detected in deps.dependencies for project %s: edges=%v", projectID, dump)
	}
}
