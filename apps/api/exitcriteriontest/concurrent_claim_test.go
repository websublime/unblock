// concurrent_claim_test.go covers the §11.1.2 + §11.3 atomic-claim
// invariant: N=100 concurrent `claim` calls against the same Ready
// item produce exactly one winner and N-1 ALREADY_CLAIMED errors.
//
// This is the MCP-tool-surface property test. The RPC-layer version
// at apps/api/workitems/integration_test.go covers workitems.Claim
// directly; this test exercises the full Bearer → Identity →
// tracectx → workitems.Claim path through the MCP transport so the
// atomic-claim invariant is verified at the production wire layer
// agents will actually hit.

package exitcriteriontest_test

import (
	"context"
	"encoding/json"
	"sync"
	"testing"

	"encore.app/shared/ulid"

	encoredb "encore.app/db"
)

// concurrentClaimN is the property-test cardinality per SPEC §11.3
// "atomic claim is a single transaction with SELECT FOR UPDATE
// (property test: N=100 concurrent claim attempts on the same item;
// assert exactly one winner and N-1 ALREADY_CLAIMED errors)".
const concurrentClaimN = 100

// TestExitCriterion_ConcurrentClaimSingleWinner seeds a fresh Ready
// item under the exit-criterion org/project, opens N=100 fresh MCP
// sessions on the same Bearer, fires `claim` concurrently from
// every session, and asserts exactly one returns claimed=true while
// the remaining N-1 return the §7 ALREADY_CLAIMED envelope.
//
// Fresh item per test (not itm_b from the fixture) because the
// prime/ready/claim/close flow above already mutated itm_b to Done.
// The §6.4 transaction asserts on the item's claimed_by_id column
// inside SELECT FOR UPDATE; running this on the shared fixture
// would force a test-ordering coupling. The item is inserted via
// direct SQL with status=Ready + is_ready=true so the seed bypasses
// the workitems.Create→Claim path.
func TestExitCriterion_ConcurrentClaimSingleWinner(t *testing.T) {
	f := fx(t)

	// Seed a fresh Ready item under the same org/project as the
	// fixture's Bearer. The Bearer's Identity.OrgID gates every
	// rbac.For read; the item must carry the matching org_id or
	// every claim attempt will fail with NotFound (not the
	// ALREADY_CLAIMED we are trying to provoke).
	targetID, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	if _, err := encoredb.DB.Exec(t.Context(),
		`INSERT INTO workitems.items
		   (id, org_id, project_id, type, title, status, is_ready)
		 VALUES ($1, $2, $3, 'task', $4, 'Ready', true)`,
		targetID, f.OrgID, f.ProjectID, "concurrent-claim-target",
	); err != nil {
		t.Fatalf("insert target item: %v", err)
	}
	t.Cleanup(func() {
		// Use context.Background() (NOT t.Context()) — t.Context()
		// is cancelled by the time t.Cleanup fires, so a DELETE
		// passed t.Context() fails with "context canceled" and the
		// claimed row leaks across tests (observed in prime's
		// claimed_by_me when this test preceded
		// PrimeReadyClaimCloseCascadeFlow).
		_, _ = encoredb.DB.Exec(context.Background(), `DELETE FROM workitems.items WHERE id = $1`, targetID)
	})

	// Pre-warm N sessions on the same Bearer. The SDK's stateful
	// session map keys each session_id independently; mint
	// concurrentClaimN distinct sessions so the contending goroutines
	// hit N independent session paths through the SDK (mirroring the
	// real-world "N agents racing on the same Ready item" shape).
	sessions := make([]string, concurrentClaimN)
	for i := 0; i < concurrentClaimN; i++ {
		sessions[i] = initializeSession(t, f.RawKey)
	}

	// Fire all N claims concurrently. Each goroutine records its
	// outcome into the per-index slot; the main goroutine then
	// tallies winners vs ALREADY_CLAIMED errors.
	type outcome struct {
		Claimed       bool
		ErrKind       string
		ErrCode       int
		StructuredRaw []byte
	}
	outcomes := make([]outcome, concurrentClaimN)

	var wg sync.WaitGroup
	wg.Add(concurrentClaimN)
	startGate := make(chan struct{})

	for i := 0; i < concurrentClaimN; i++ {
		i := i
		go func() {
			defer wg.Done()
			<-startGate

			env := callTool(t, f.RawKey, sessions[i], "claim", map[string]any{
				"item_id": targetID,
			})

			if env.Error != nil {
				var data envelopeData
				_ = json.Unmarshal(env.Error.Data, &data)
				outcomes[i] = outcome{
					Claimed: false,
					ErrKind: data.Kind,
					ErrCode: env.Error.Code,
				}
				return
			}
			// Success path: parse the structured payload.
			var res toolCallResult
			if err := json.Unmarshal(env.Result, &res); err != nil {
				outcomes[i] = outcome{ErrKind: "PARSE_RESULT"}
				return
			}
			var c struct {
				Claimed bool `json:"claimed"`
			}
			_ = json.Unmarshal(res.StructuredContent, &c)
			outcomes[i] = outcome{
				Claimed:       c.Claimed,
				StructuredRaw: res.StructuredContent,
			}
		}()
	}

	close(startGate) // release all goroutines at once
	wg.Wait()

	// Tally: exactly one winner + N-1 ALREADY_CLAIMED.
	var winners, alreadyClaimed, other int
	for _, o := range outcomes {
		switch {
		case o.Claimed:
			winners++
		case o.ErrKind == "ALREADY_CLAIMED":
			alreadyClaimed++
		default:
			other++
		}
	}
	if winners != 1 {
		t.Fatalf("winners = %d, want exactly 1; outcomes=%+v", winners, outcomes)
	}
	if alreadyClaimed != concurrentClaimN-1 {
		t.Fatalf("ALREADY_CLAIMED = %d, want %d (N-1); outcomes=%+v", alreadyClaimed, concurrentClaimN-1, outcomes)
	}
	if other != 0 {
		t.Fatalf("unexpected non-claim non-ALREADY_CLAIMED outcomes: %d", other)
	}
}
