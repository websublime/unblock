// Integration-style tests for the cascade subscriber, calling
// handleCascadeRequested directly. Encore's pubsub testing
// implementation does NOT fire subscriptions during `encore test`
// (https://encore.dev/docs/go/primitives/pubsub#testing-pubsub —
// "Your subscriptions will not be triggered by events published. This
// allows you to test the behaviour of publishers independently of side
// effects caused by subscribers."), so these tests invoke the handler
// directly. Same handler, same DB writes, same idempotency contract —
// only the Pub/Sub fan-out is skipped, and that fan-out is exercised
// in production (encore run) where the subscriber is wired by the
// pubsub.NewSubscription call in cascade_subscriber.go.
//
// This is an internal-package test (package deps) so the unexported
// handleCascadeRequested function is reachable. The db handle is bound
// by the encore.app/db service's init() during encore test bootstrap;
// we do NOT import encore.app/db here (it would form a back-import
// cycle: db imports deps).

package deps

import (
	"context"
	"strings"
	"sync"
	"testing"
	"time"

	"encore.app/shared/ulid"
)

// seedFixtureInternal mirrors deps_test.seedFixture but lives in
// package deps so an internal-package test can use it without the
// back-import cycle. Inserts an org, a user, and a project; returns
// the ids and registers cleanup.
type internalFixture struct {
	OrgID     string
	ProjectID string
	UserID    string
}

func seedFixtureInternal(t *testing.T, ctx context.Context) *internalFixture {
	t.Helper()
	if db == nil {
		t.Fatalf("deps.db is nil — encore.app/db init did not bind the handle (run via encore test)")
	}

	orgID := mustULIDInternal(t)
	slug := strings.ToLower("ct-" + orgID[len(orgID)-8:])
	if _, err := db.Exec(ctx,
		`INSERT INTO org.organizations (id, slug, name) VALUES ($1, $2, $3)`,
		orgID, slug, "cascade test org",
	); err != nil {
		t.Fatalf("insert org: %v", err)
	}

	userID := mustULIDInternal(t)
	if _, err := db.Exec(ctx,
		`INSERT INTO auth.users (id, primary_provider, primary_provider_id, email, display_name)
		 VALUES ($1, 'github', $2, $3, $4)`,
		userID, "ct-"+userID[len(userID)-8:],
		strings.ToLower(userID[len(userID)-8:])+"@ct.local", "ct",
	); err != nil {
		t.Fatalf("insert user: %v", err)
	}

	projectID := mustULIDInternal(t)
	if _, err := db.Exec(ctx,
		`INSERT INTO org.projects (id, org_id, slug, name) VALUES ($1, $2, $3, $4)`,
		projectID, orgID, "p-"+projectID[len(projectID)-8:], "ct project",
	); err != nil {
		t.Fatalf("insert project: %v", err)
	}

	t.Cleanup(func() {
		_, _ = db.Exec(ctx, `DELETE FROM org.organizations WHERE id = $1`, orgID)
		_, _ = db.Exec(ctx, `DELETE FROM auth.users WHERE id = $1`, userID)
	})
	return &internalFixture{OrgID: orgID, ProjectID: projectID, UserID: userID}
}

func mustULIDInternal(t *testing.T) string {
	t.Helper()
	id, err := ulid.New()
	if err != nil {
		t.Fatalf("ulid: %v", err)
	}
	return id
}

// createItemInternal inserts a Backlog item. Returns the id. Items
// default to is_ready=true; tests that need is_ready=false create
// blocking edges via deps.AddEdge.
func createItemInternal(t *testing.T, ctx context.Context, fx *internalFixture, status string) string {
	t.Helper()
	if status == "" {
		status = "Backlog"
	}
	id := mustULIDInternal(t)
	if _, err := db.Exec(ctx,
		`INSERT INTO workitems.items
		   (id, org_id, project_id, type, title, status, is_ready)
		 VALUES ($1, $2, $3, 'task', 'ct item', $4, true)`,
		id, fx.OrgID, fx.ProjectID, status,
	); err != nil {
		t.Fatalf("insert item: %v", err)
	}
	return id
}

// readPipelineStageInternal reads workitems.items.pipeline_stage.
func readPipelineStageInternal(t *testing.T, ctx context.Context, id string) string {
	t.Helper()
	var stage string
	if err := db.QueryRow(ctx,
		`SELECT pipeline_stage FROM workitems.items WHERE id = $1`, id,
	).Scan(&stage); err != nil {
		t.Fatalf("read pipeline_stage: %v", err)
	}
	return stage
}

// readIsReadyInternal reads workitems.items.is_ready.
func readIsReadyInternal(t *testing.T, ctx context.Context, id string) bool {
	t.Helper()
	var ready bool
	if err := db.QueryRow(ctx,
		`SELECT is_ready FROM workitems.items WHERE id = $1`, id,
	).Scan(&ready); err != nil {
		t.Fatalf("read is_ready: %v", err)
	}
	return ready
}

// countCascadeEvents counts deps.cascade_events rows by event_id +
// triggered_by_item_id (the idempotency key).
func countCascadeEvents(t *testing.T, ctx context.Context, eventID, triggeredBy string) int {
	t.Helper()
	var n int
	if err := db.QueryRow(ctx,
		`SELECT count(*) FROM deps.cascade_events
		   WHERE event_id = $1 AND triggered_by_item_id = $2`,
		eventID, triggeredBy,
	).Scan(&n); err != nil {
		t.Fatalf("count cascade_events: %v", err)
	}
	return n
}

// TestHandleCascadeRequested_FourKinds calls the subscriber handler
// directly with one CascadeRequested message per Reason. Asserts:
//   - exactly one deps.cascade_events row is written with kind = msg.Reason.
//   - the affected_item_ids array contains the seed (BFS includes the seed).
//   - trace_id round-trips from msg.TraceID to the column.
//   - subscriber does NOT write is_ready (the seed's is_ready stays at
//     its pre-call value).
func TestHandleCascadeRequested_FourKinds(t *testing.T) {
	ctx := context.Background()
	fx := seedFixtureInternal(t, ctx)

	for _, reason := range []string{"close", "edge_added", "edge_removed", "state_change"} {
		t.Run(reason, func(t *testing.T) {
			seed := createItemInternal(t, ctx, fx, "Backlog")
			eventID := mustULIDInternal(t)
			traceID := mustULIDInternal(t)

			msg := &CascadeRequested{
				EventID:           eventID,
				OrgID:             fx.OrgID,
				ProjectID:         fx.ProjectID,
				TriggeredByItemID: seed,
				Reason:            reason,
				TraceID:           traceID,
				EmittedAt:         time.Now().UTC(),
			}

			preReady := readIsReadyInternal(t, ctx, seed)

			if err := handleCascadeRequested(ctx, msg); err != nil {
				t.Fatalf("handleCascadeRequested(%s): %v", reason, err)
			}

			// Exactly one audit row with kind=msg.Reason.
			var kind string
			var traceColumn *string
			var affected []string
			if err := db.QueryRow(ctx,
				`SELECT kind, trace_id, affected_item_ids
				   FROM deps.cascade_events
				  WHERE event_id = $1 AND triggered_by_item_id = $2`,
				eventID, seed,
			).Scan(&kind, &traceColumn, &affected); err != nil {
				t.Fatalf("read cascade_events: %v", err)
			}
			if kind != reason {
				t.Fatalf("kind = %q, want %q", kind, reason)
			}
			if traceColumn == nil || *traceColumn != traceID {
				t.Fatalf("trace_id = %v, want %q", traceColumn, traceID)
			}
			seenSeed := false
			for _, id := range affected {
				if id == seed {
					seenSeed = true
					break
				}
			}
			if !seenSeed {
				t.Fatalf("affected_item_ids missing seed %q: %v", seed, affected)
			}

			// Regime A invariant: subscriber MUST NOT write is_ready.
			postReady := readIsReadyInternal(t, ctx, seed)
			if postReady != preReady {
				t.Fatalf("subscriber wrote is_ready (%v → %v); SPEC §11.3 bullet (b) violated", preReady, postReady)
			}
		})
	}
}

// TestHandleCascadeRequested_IdempotencyN100 invokes the handler 100
// times with the SAME (event_id, triggered_by_item_id) tuple across
// all four kinds (25 per kind, but the kind column is set by the FIRST
// write that wins the ON CONFLICT race — subsequent calls collapse to
// no-ops). Asserts exactly one row exists after all 100 calls.
//
// SPEC §6.3.2 line 1828 + AR-11 — the (event_id, triggered_by_item_id)
// UNIQUE constraint plus ON CONFLICT DO NOTHING is the structural
// idempotency mechanism. AR-11 / acceptance criterion #2 of this bead.
func TestHandleCascadeRequested_IdempotencyN100(t *testing.T) {
	ctx := context.Background()
	fx := seedFixtureInternal(t, ctx)
	seed := createItemInternal(t, ctx, fx, "Backlog")

	const total = 100
	kinds := []string{"close", "edge_added", "edge_removed", "state_change"}
	eventID := mustULIDInternal(t)
	traceID := mustULIDInternal(t)

	for i := 0; i < total; i++ {
		k := kinds[i%len(kinds)]
		msg := &CascadeRequested{
			EventID:           eventID,
			OrgID:             fx.OrgID,
			ProjectID:         fx.ProjectID,
			TriggeredByItemID: seed,
			Reason:            k,
			TraceID:           traceID,
			EmittedAt:         time.Now().UTC(),
		}
		if err := handleCascadeRequested(ctx, msg); err != nil {
			t.Fatalf("handle %d (%s): %v", i, k, err)
		}
	}

	got := countCascadeEvents(t, ctx, eventID, seed)
	if got != 1 {
		t.Fatalf("AR-11 idempotency violation: %d rows after %d redeliveries, want 1", got, total)
	}
}

// TestHandleCascadeRequested_IdempotentPipelineStage triggers two
// passes of Reason='close' with DIFFERENT event_ids (so each invokes
// the subscriber body, but the per-item UPDATE …  WHERE pipeline_stage
// <> $new short-circuits on the second pass). Asserts pipeline_stage
// is stable across both passes (idempotent value-equality write).
func TestHandleCascadeRequested_IdempotentPipelineStage(t *testing.T) {
	ctx := context.Background()
	fx := seedFixtureInternal(t, ctx)
	seed := createItemInternal(t, ctx, fx, "Backlog")

	runOnce := func() {
		msg := &CascadeRequested{
			EventID:           mustULIDInternal(t),
			OrgID:             fx.OrgID,
			ProjectID:         fx.ProjectID,
			TriggeredByItemID: seed,
			Reason:            "close",
			EmittedAt:         time.Now().UTC(),
		}
		if err := handleCascadeRequested(ctx, msg); err != nil {
			t.Fatalf("handle: %v", err)
		}
	}

	runOnce()
	first := readPipelineStageInternal(t, ctx, seed)
	runOnce()
	second := readPipelineStageInternal(t, ctx, seed)

	if first != second {
		t.Fatalf("pipeline_stage flipped between passes: %q → %q (idempotency violation)", first, second)
	}
}

// TestHandleCascadeRequested_MultiHopBFS exercises the forward 'blocks'
// closure walk: build a→b→c (a blocks b, b blocks c), trigger
// Reason='edge_added' from a. The BFS from a must reach b AND c (depth
// 2), and the audit row's affected_item_ids must include all three.
func TestHandleCascadeRequested_MultiHopBFS(t *testing.T) {
	ctx := context.Background()
	fx := seedFixtureInternal(t, ctx)

	a := createItemInternal(t, ctx, fx, "Backlog")
	b := createItemInternal(t, ctx, fx, "Backlog")
	c := createItemInternal(t, ctx, fx, "Backlog")

	// Use the same SQL deps.AddEdge would write, bypassing the RPC so
	// we don't fire the publisher (we want to test BFS, not the
	// publisher path).
	for _, pair := range [][2]string{{a, b}, {b, c}} {
		edgeID := mustULIDInternal(t)
		if _, err := db.Exec(ctx,
			`INSERT INTO deps.dependencies (id, from_item, to_item, kind)
			 VALUES ($1, $2, $3, 'blocks')`,
			edgeID, pair[0], pair[1],
		); err != nil {
			t.Fatalf("insert edge %s->%s: %v", pair[0], pair[1], err)
		}
	}

	eventID := mustULIDInternal(t)
	msg := &CascadeRequested{
		EventID:           eventID,
		OrgID:             fx.OrgID,
		ProjectID:         fx.ProjectID,
		TriggeredByItemID: a,
		Reason:            "edge_added",
		EmittedAt:         time.Now().UTC(),
	}
	if err := handleCascadeRequested(ctx, msg); err != nil {
		t.Fatalf("handle: %v", err)
	}

	var affected []string
	if err := db.QueryRow(ctx,
		`SELECT affected_item_ids FROM deps.cascade_events
		  WHERE event_id = $1 AND triggered_by_item_id = $2`,
		eventID, a,
	).Scan(&affected); err != nil {
		t.Fatalf("read affected_item_ids: %v", err)
	}

	want := map[string]bool{a: true, b: true, c: true}
	got := map[string]bool{}
	for _, id := range affected {
		got[id] = true
	}
	for id := range want {
		if !got[id] {
			t.Fatalf("affected missing %q: got=%v", id, affected)
		}
	}
}

// TestHandleCascadeRequested_UnknownReason asserts the defensive
// dispatch drops unknown Reason values without writing an audit row
// (per SPEC §6.3.2 line 1806-1810 — "Unknown Reason — log + drop").
func TestHandleCascadeRequested_UnknownReason(t *testing.T) {
	ctx := context.Background()
	fx := seedFixtureInternal(t, ctx)
	seed := createItemInternal(t, ctx, fx, "Backlog")

	eventID := mustULIDInternal(t)
	msg := &CascadeRequested{
		EventID:           eventID,
		OrgID:             fx.OrgID,
		ProjectID:         fx.ProjectID,
		TriggeredByItemID: seed,
		Reason:            "not_a_real_kind",
		EmittedAt:         time.Now().UTC(),
	}
	if err := handleCascadeRequested(ctx, msg); err != nil {
		t.Fatalf("handle (unknown reason): %v", err)
	}
	if got := countCascadeEvents(t, ctx, eventID, seed); got != 0 {
		t.Fatalf("unknown reason wrote audit row: count=%d", got)
	}
}

// TestHandleCascadeRequested_EmptyTrace asserts that a publisher with
// empty TraceID (admin scripts, seeders) lands NULL in
// deps.cascade_events.trace_id rather than an empty string.
func TestHandleCascadeRequested_EmptyTrace(t *testing.T) {
	ctx := context.Background()
	fx := seedFixtureInternal(t, ctx)
	seed := createItemInternal(t, ctx, fx, "Backlog")

	eventID := mustULIDInternal(t)
	msg := &CascadeRequested{
		EventID:           eventID,
		OrgID:             fx.OrgID,
		ProjectID:         fx.ProjectID,
		TriggeredByItemID: seed,
		Reason:            "close",
		TraceID:           "", // empty — non-MCP publisher
		EmittedAt:         time.Now().UTC(),
	}
	if err := handleCascadeRequested(ctx, msg); err != nil {
		t.Fatalf("handle: %v", err)
	}

	var traceColumn *string
	if err := db.QueryRow(ctx,
		`SELECT trace_id FROM deps.cascade_events
		  WHERE event_id = $1 AND triggered_by_item_id = $2`,
		eventID, seed,
	).Scan(&traceColumn); err != nil {
		t.Fatalf("read trace_id: %v", err)
	}
	if traceColumn != nil {
		t.Fatalf("trace_id = %q, want NULL (empty TraceID input)", *traceColumn)
	}
}

// TestHandleCascadeRequested_DerivesPipelineStage covers an end-to-end
// derivation path: seed with impl_state='done', review_state='pending'
// AND a kind='review' comment on the item — §5.7.1 rule 9 → pipeline_stage='Review'.
// Confirms the subscriber's batched comment-existence query feeds the
// derivation correctly.
func TestHandleCascadeRequested_DerivesPipelineStage(t *testing.T) {
	ctx := context.Background()
	fx := seedFixtureInternal(t, ctx)
	seed := createItemInternal(t, ctx, fx, "Backlog")

	// Move the seed into the "review queued" shape: impl_state='done',
	// review_state='pending', and a kind='review' comment exists.
	if _, err := db.Exec(ctx,
		`UPDATE workitems.items
		    SET impl_state = 'done', review_state = 'pending', qa_state = 'pending'
		  WHERE id = $1`,
		seed,
	); err != nil {
		t.Fatalf("set states: %v", err)
	}
	commentID := mustULIDInternal(t)
	if _, err := db.Exec(ctx,
		`INSERT INTO workitems.comments
		   (id, item_id, author_id, kind, status, body)
		 VALUES ($1, $2, $3, 'review', 'info', 'looks good')`,
		commentID, seed, fx.UserID,
	); err != nil {
		t.Fatalf("insert review comment: %v", err)
	}

	msg := &CascadeRequested{
		EventID:           mustULIDInternal(t),
		OrgID:             fx.OrgID,
		ProjectID:         fx.ProjectID,
		TriggeredByItemID: seed,
		Reason:            "state_change",
		EmittedAt:         time.Now().UTC(),
	}
	if err := handleCascadeRequested(ctx, msg); err != nil {
		t.Fatalf("handle: %v", err)
	}

	got := readPipelineStageInternal(t, ctx, seed)
	if got != "Review" {
		t.Fatalf("pipeline_stage = %q, want %q (rule 9)", got, "Review")
	}
}

// TestHandleCascadeRequested_BFS_TenantIsolation locks the
// defence-in-depth tenant predicate added in unblock-tv8.50.
//
// Setup: two distinct orgs (alpha, beta). Each org has one item.
// We bypass deps.AddEdge (which rejects cross-org edges upstream)
// and INSERT a cross-tenant 'blocks' row directly into
// deps.dependencies via raw SQL: alpha_seed → beta_neighbour.
//
// Trigger: publish CascadeRequested with OrgID = alpha, ProjectID =
// alpha's project, TriggeredByItemID = alpha_seed, Reason = close.
//
// Assertions:
//  1. The audit row's affected_item_ids contains alpha_seed but does
//     NOT contain beta_neighbour. The BFS dropped the cross-tenant
//     edge in the recursive step because the JOIN to workitems.items
//     filters on (i.org_id = alpha.org_id).
//  2. beta_neighbour's pipeline_stage is unchanged (the subscriber
//     never wrote it — recomputePipelineStageForAffected's tenant
//     predicate would also reject it even if the BFS had leaked).
//
// This test would FAIL before unblock-tv8.50 — the BFS would walk
// the cross-tenant edge and leak alpha's cascade into beta. The
// fix gates the BFS by msg.OrgID via JOIN workitems.items and
// re-validates the tenant in the derivation read.
func TestHandleCascadeRequested_BFS_TenantIsolation(t *testing.T) {
	ctx := context.Background()
	alpha := seedFixtureInternal(t, ctx)
	beta := seedFixtureInternal(t, ctx)

	alphaSeed := createItemInternal(t, ctx, alpha, "Backlog")
	betaNeighbour := createItemInternal(t, ctx, beta, "Backlog")

	// Capture beta_neighbour's pipeline_stage BEFORE the cascade so
	// we can confirm the subscriber never wrote it.
	betaStageBefore := readPipelineStageInternal(t, ctx, betaNeighbour)

	// Raw SQL bypass: deps.AddEdge would reject this with
	// "cross-org edges are not allowed" (deps.go:247-257). The
	// test's whole point is to simulate the latent gap — a row that
	// somehow exists in deps.dependencies spanning two tenants
	// (e.g. via a future writer-path regression or a direct DDL
	// bypass).
	edgeID := mustULIDInternal(t)
	if _, err := db.Exec(ctx,
		`INSERT INTO deps.dependencies (id, from_item, to_item, kind)
		 VALUES ($1, $2, $3, 'blocks')`,
		edgeID, alphaSeed, betaNeighbour,
	); err != nil {
		t.Fatalf("insert cross-tenant edge: %v", err)
	}

	eventID := mustULIDInternal(t)
	msg := &CascadeRequested{
		EventID:           eventID,
		OrgID:             alpha.OrgID,
		ProjectID:         alpha.ProjectID,
		TriggeredByItemID: alphaSeed,
		Reason:            "close",
		EmittedAt:         time.Now().UTC(),
	}
	if err := handleCascadeRequested(ctx, msg); err != nil {
		t.Fatalf("handle: %v", err)
	}

	// Read the audit row's affected_item_ids.
	var affected []string
	if err := db.QueryRow(ctx,
		`SELECT affected_item_ids FROM deps.cascade_events
		  WHERE event_id = $1 AND triggered_by_item_id = $2`,
		eventID, alphaSeed,
	).Scan(&affected); err != nil {
		t.Fatalf("read affected_item_ids: %v", err)
	}

	// Assertion 1: alpha_seed must be IN the set (BFS includes the
	// seed when its tenant matches the publisher).
	seenAlpha := false
	for _, id := range affected {
		if id == alphaSeed {
			seenAlpha = true
		}
		if id == betaNeighbour {
			t.Fatalf("BFS leaked across tenants: affected_item_ids contains beta_neighbour %q; alpha=%q, affected=%v",
				betaNeighbour, alphaSeed, affected)
		}
	}
	if !seenAlpha {
		t.Fatalf("alpha_seed %q missing from affected_item_ids: %v", alphaSeed, affected)
	}

	// Assertion 2: beta_neighbour's pipeline_stage must be unchanged.
	betaStageAfter := readPipelineStageInternal(t, ctx, betaNeighbour)
	if betaStageAfter != betaStageBefore {
		t.Fatalf("subscriber wrote beta_neighbour.pipeline_stage across tenants: %q → %q",
			betaStageBefore, betaStageAfter)
	}
}

// TestHandleCascadeRequested_BFS_TenantMismatch covers the symmetric
// edge case: publisher's OrgID disagrees with the seed item's actual
// org_id. The anchor SELECT in the recursive CTE filters on
// i.org_id = msg.OrgID, so the BFS returns an empty result. The
// audit row is still written (the publisher's claim is preserved
// for forensics) but affected_item_ids is empty and no pipeline_stage
// write occurs anywhere.
//
// This locks the documented behaviour in bfsForwardBlocksClosure's
// doc comment: "If the publisher's (orgID, projectID) disagrees with
// the seed's actual row, the anchor SELECT returns no rows ... The
// audit row INSERT in insertCascadeEventRow still writes with
// msg.OrgID as authoritative — the audit captures what was REQUESTED,
// not what was REACHABLE."
func TestHandleCascadeRequested_BFS_TenantMismatch(t *testing.T) {
	ctx := context.Background()
	alpha := seedFixtureInternal(t, ctx)
	beta := seedFixtureInternal(t, ctx)

	// Seed lives in alpha; publisher claims beta.
	seed := createItemInternal(t, ctx, alpha, "Backlog")
	stageBefore := readPipelineStageInternal(t, ctx, seed)

	eventID := mustULIDInternal(t)
	msg := &CascadeRequested{
		EventID:           eventID,
		OrgID:             beta.OrgID, // disagrees with seed's actual org
		ProjectID:         beta.ProjectID,
		TriggeredByItemID: seed,
		Reason:            "close",
		EmittedAt:         time.Now().UTC(),
	}
	if err := handleCascadeRequested(ctx, msg); err != nil {
		t.Fatalf("handle (tenant mismatch): %v", err)
	}

	// Audit row exists, kind = 'close', affected_item_ids is empty.
	// Lock the "audit captures REQUESTED, not REACHABLE" contract
	// (cascade_subscriber.go:415-427 invariant + bfsForwardBlocksClosure
	// doc comment): the audit row's org_id MUST be the publisher's
	// claim (beta.OrgID), NOT the seed item's actual org_id (alpha.OrgID).
	type auditRow struct {
		kind      string
		orgID     string
		projectID string
		affected  []string
	}
	var got auditRow
	if err := db.QueryRow(ctx,
		`SELECT kind, org_id, project_id, affected_item_ids
		   FROM deps.cascade_events
		  WHERE event_id = $1 AND triggered_by_item_id = $2`,
		eventID, seed,
	).Scan(&got.kind, &got.orgID, &got.projectID, &got.affected); err != nil {
		t.Fatalf("read cascade_events: %v", err)
	}
	if got.kind != "close" {
		t.Fatalf("kind = %q, want %q (audit captures what was requested)", got.kind, "close")
	}
	if len(got.affected) != 0 {
		t.Fatalf("affected_item_ids non-empty under tenant mismatch: %v", got.affected)
	}
	// Audit org_id must equal publisher's claim (beta), not seed's
	// actual tenant (alpha). This is the locked contract: the audit
	// captures REQUESTED, not REACHABLE — the publisher's claim
	// remains visible in the audit even when the BFS walk yields
	// nothing because the seed lives in a different tenant.
	if got.orgID != beta.OrgID {
		t.Fatalf("audit org_id = %q, want publisher's claim %q (NOT seed's actual org %q) — audit must capture REQUESTED, not REACHABLE",
			got.orgID, beta.OrgID, alpha.OrgID)
	}
	if got.orgID == alpha.OrgID {
		t.Fatalf("audit org_id leaked seed's actual tenant %q — contract violated, audit must reflect publisher's claim %q",
			alpha.OrgID, beta.OrgID)
	}

	// Seed's pipeline_stage must be unchanged — the subscriber's
	// derivation read filtered it out too.
	stageAfter := readPipelineStageInternal(t, ctx, seed)
	if stageAfter != stageBefore {
		t.Fatalf("subscriber wrote across tenant mismatch: pipeline_stage %q → %q", stageBefore, stageAfter)
	}
}

// TestHandleCascadeRequested_ConcurrentLWWRace exercises the
// unblock-tv8.51 LWW race fix in recomputePipelineStageForAffected.
//
// REGRESSION-DETECTION SHAPE (review hardening unblock-tv8.51 follow-up):
//
// The race the bead names is between TWO concurrent cascade subscriber
// invocations operating on the SAME item, where the input state for the
// §5.7.1 derivation flips BETWEEN the SELECT of one handler and its
// UPDATE. Without the tx + FOR NO KEY UPDATE fix, the slower handler's
// UPDATE can clobber a fresher value with a stale derivation.
//
// To exercise that window, two things must be true simultaneously:
//   - At least one handler reads pre-mutation inputs and derives stage
//     A (which differs from b's current pipeline_stage, so the UPDATE
//     actually fires — the value-equality guard `pipeline_stage <> $2`
//     would otherwise short-circuit and there is no race to detect).
//   - The mutation happens DURING the concurrent window (between
//     handlers), and at least one handler reads post-mutation inputs
//     and derives stage B (also different from b's current value).
//
// Setup:
//   - Chain a→b→c so any cascade from a walks the closure {a, b, c}.
//   - b in rule-9 shape (impl_state='done', review_state='pending',
//     qa_state='pending', has a kind='review' comment) → derives Review.
//   - PRE-SEED b.pipeline_stage = 'Investigation' so the FIRST handler
//     write is a real UPDATE (not a no-op via the value-equality guard).
//     This is the non-obvious detail the original test missed: identical
//     pre-state with all derivations matching the current value makes
//     every UPDATE a no-op, and a no-op cannot be clobbered.
//
// Race driver (interleaved waves under a barrier):
//   - Wave G1 (concurrency/2 goroutines): handleCascadeRequested with
//     UNIQUE event_ids. Each handler reads b's inputs, derives the
//     current correct stage, and UPDATEs if it differs.
//   - Mutator goroutine (launched alongside G1, on the same barrier):
//     flips b into the rule-7 shape (review_state='approved',
//     qa_state='pending') via a direct UPDATE — simulates Tool 13
//     SetStateColumns. The correct derivation for b is now Quality.
//   - Wave G2 (concurrency/2 goroutines): handleCascadeRequested with
//     UNIQUE event_ids, launched AFTER the mutator releases (gate2).
//     Each reads b's POST-mutation inputs and derives Quality.
//
// What the row lock buys (with the fix):
//   - All handlers' items SELECT + UPDATE pair runs inside one tx that
//     holds FOR NO KEY UPDATE on b. The mutator's UPDATE acquires a
//     conflicting ROW EXCLUSIVE lock and serialises against the
//     handlers. Some handler ordering is possible (the lock queue is
//     FIFO-ish but not strictly), but EVERY commit reads a snapshot
//     consistent with the row state at the moment its lock was granted.
//   - Therefore the FINAL committed pipeline_stage MUST equal a fresh
//     re-derivation of b's FINAL committed inputs. The last lock-holder
//     by definition reads the latest committed state.
//
// What breaks without the fix:
//   - A G1 handler can SELECT b in pre-mutation state, derive Review,
//     pause in Go memory while the mutator commits review_state=
//     'approved', then UPDATE pipeline_stage='Review'. A G2 handler
//     correctly derives and writes Quality. If the G1 UPDATE commits
//     LAST, the final pipeline_stage is the stale 'Review' even though
//     the §5.7.1 derivation from the final committed inputs is Quality.
//   - The assertion `finalStage == derivePipelineStage(final inputs)`
//     fails on that interleaving.
//
// Statistical confidence:
//   - Goroutine launch order is non-deterministic; even with the gate
//     barrier the OS may park some goroutines before the SELECT lands.
//   - The harness wraps the body in a `for run := 0; run < runs; run++`
//     loop so a single `go test -run TestHandleCascadeRequested_
//     ConcurrentLWWRace` invocation exercises `runs` independent
//     interleavings against fresh fixtures. Combined with the recommended
//     `-count=5` (or `-count=10`) on the test invocation itself, the
//     suite explores >50 interleavings per CI run.
//   - With the tx wrap in place: 100% pass across all interleavings.
//   - Without the tx wrap (revert experiment, NOT landed): a non-trivial
//     fraction (~>0%) of interleavings would fail the post-state
//     assertion. The worker confirmed this locally during fix bring-up
//     (round-11 review hardening); the revert is NOT shipped — only the
//     test shape is.
//
// The test does NOT rely on the Go race detector (round-11 NFR-10
// dropped -race from the encore-side gate for packages that import
// encore.dev/pubsub at init); it relies on observable post-state
// correctness re-derived from the final committed inputs.
func TestHandleCascadeRequested_ConcurrentLWWRace(t *testing.T) {
	ctx := context.Background()
	fx := seedFixtureInternal(t, ctx)

	// runsPerInvocation runs the race body multiple times per test
	// invocation. Combined with `-count=N` on the test command this
	// gives `runs * N` independent interleavings, enough to flush
	// timing variance.
	const runsPerInvocation = 5

	for run := 0; run < runsPerInvocation; run++ {
		// Build chain a→b→c per run so each iteration races against a
		// fresh fixture and a stale prior-run state cannot mask a bug.
		a := createItemInternal(t, ctx, fx, "Backlog")
		b := createItemInternal(t, ctx, fx, "Backlog")
		c := createItemInternal(t, ctx, fx, "Backlog")
		for _, pair := range [][2]string{{a, b}, {b, c}} {
			edgeID := mustULIDInternal(t)
			if _, err := db.Exec(ctx,
				`INSERT INTO deps.dependencies (id, from_item, to_item, kind)
				 VALUES ($1, $2, $3, 'blocks')`,
				edgeID, pair[0], pair[1],
			); err != nil {
				t.Fatalf("run %d: insert edge %s->%s: %v", run, pair[0], pair[1], err)
			}
		}

		// Initial derivation inputs for b — rule 9 (Review) shape.
		// Pre-seed pipeline_stage='Investigation' so the FIRST cascade
		// pass is a REAL UPDATE (not a no-op via WHERE pipeline_stage
		// <> $2). Without this, every handler's UPDATE is a no-op and
		// the race window cannot be exercised — that was the WARNING
		// from the round-11 review against the original test shape.
		if _, err := db.Exec(ctx,
			`UPDATE workitems.items
			    SET impl_state = 'done', review_state = 'pending', qa_state = 'pending',
			        pipeline_stage = 'Investigation'
			  WHERE id = $1`,
			b,
		); err != nil {
			t.Fatalf("run %d: set initial b state: %v", run, err)
		}
		reviewCommentID := mustULIDInternal(t)
		if _, err := db.Exec(ctx,
			`INSERT INTO workitems.comments
			   (id, item_id, author_id, kind, status, body)
			 VALUES ($1, $2, $3, 'review', 'info', 'looks good')`,
			reviewCommentID, b, fx.UserID,
		); err != nil {
			t.Fatalf("run %d: insert review comment on b: %v", run, err)
		}

		// Concurrent waves under a shared barrier.
		//   - gate1: releases wave G1 + the mutator simultaneously.
		//   - gate2: released by the mutator after its UPDATE commits;
		//     releases wave G2 so its handlers read the post-mutation
		//     inputs.
		// The mutator landing BETWEEN waves (not after the suite
		// finishes) is the difference from the original test shape.
		const concurrency = 16
		const waveSize = concurrency / 2
		var (
			ready1   sync.WaitGroup
			ready2   sync.WaitGroup
			gate1    sync.WaitGroup
			gate2    sync.WaitGroup
			done     sync.WaitGroup
			mu       sync.Mutex
			errs     []error
			mutateOK bool
		)
		ready1.Add(waveSize + 1) // wave G1 + mutator park at gate1
		ready2.Add(waveSize)     // wave G2 parks at gate2
		gate1.Add(1)
		gate2.Add(1)
		done.Add(concurrency + 1) // both waves + mutator

		// Wave G1: handlers that race against the mutator. These read
		// b's PRE-mutation inputs (rule 9 → Review) but may complete
		// their UPDATE after the mutator commits — exactly the LWW
		// window the fix targets.
		for i := 0; i < waveSize; i++ {
			go func(i int) {
				defer done.Done()
				ready1.Done()
				gate1.Wait()
				msg := &CascadeRequested{
					EventID:           mustULIDInternal(t),
					OrgID:             fx.OrgID,
					ProjectID:         fx.ProjectID,
					TriggeredByItemID: a,
					Reason:            "state_change",
					EmittedAt:         time.Now().UTC(),
				}
				if err := handleCascadeRequested(ctx, msg); err != nil {
					mu.Lock()
					errs = append(errs, err)
					mu.Unlock()
				}
			}(i)
		}

		// Mutator: simulates Tool 13 SetStateColumns flipping b into
		// the rule-7 shape (review_state='approved', qa_state='pending'
		// → Quality). Launched on the same barrier as wave G1 so the
		// mutation races against G1's handlers on b's row lock.
		go func() {
			defer done.Done()
			ready1.Done()
			gate1.Wait()
			if _, err := db.Exec(ctx,
				`UPDATE workitems.items
				    SET review_state = 'approved', qa_state = 'pending'
				  WHERE id = $1`,
				b,
			); err != nil {
				mu.Lock()
				errs = append(errs, err)
				mu.Unlock()
				gate2.Done() // unblock wave G2 even on failure so done.Wait() doesn't hang
				return
			}
			mu.Lock()
			mutateOK = true
			mu.Unlock()
			gate2.Done()
		}()

		// Wave G2: handlers that read POST-mutation inputs (rule 7 →
		// Quality). Released by the mutator's gate2.
		for i := 0; i < waveSize; i++ {
			go func(i int) {
				defer done.Done()
				ready2.Done()
				gate2.Wait()
				msg := &CascadeRequested{
					EventID:           mustULIDInternal(t),
					OrgID:             fx.OrgID,
					ProjectID:         fx.ProjectID,
					TriggeredByItemID: a,
					Reason:            "state_change",
					EmittedAt:         time.Now().UTC(),
				}
				if err := handleCascadeRequested(ctx, msg); err != nil {
					mu.Lock()
					errs = append(errs, err)
					mu.Unlock()
				}
			}(i)
		}

		// Park until every goroutine is at its barrier, then fire.
		ready1.Wait()
		ready2.Wait()
		gate1.Done()
		done.Wait()

		if len(errs) > 0 {
			t.Fatalf("run %d: concurrent handler errors: %v", run, errs)
		}
		if !mutateOK {
			t.Fatalf("run %d: mutator did not commit; race window not exercised", run)
		}

		// Post-state assertion: b's pipeline_stage must match a fresh
		// re-derivation of b's FINAL committed inputs. With the tx +
		// FOR NO KEY UPDATE fix: the row lock serialises handlers
		// against the mutator, so the last lock-holder reads the
		// committed final inputs and derives correctly. Without the
		// fix: a G1 handler could UPDATE with the stale 'Review'
		// derivation after the mutator + G2 wave have settled on
		// 'Quality', leaving b.pipeline_stage='Review' while the final
		// inputs derive to 'Quality' — assertion fails.
		var inp itemDerivationInputs
		if err := db.QueryRow(ctx,
			`SELECT id, status, pipeline_stage,
			        impl_state, review_state, qa_state, pipeline_state,
			        (closed_at IS NOT NULL)
			   FROM workitems.items WHERE id = $1`,
			b,
		).Scan(&inp.id, &inp.status, &inp.pipelineStage,
			&inp.implState, &inp.reviewState, &inp.qaState, &inp.pipelineState,
			&inp.closedAtNotNull); err != nil {
			t.Fatalf("run %d: read final b inputs: %v", run, err)
		}
		var hasReview, hasInvestigation int
		if err := db.QueryRow(ctx,
			`SELECT
			        max(CASE WHEN kind = 'review'        THEN 1 ELSE 0 END),
			        max(CASE WHEN kind = 'investigation' THEN 1 ELSE 0 END)
			   FROM workitems.comments WHERE item_id = $1`,
			b,
		).Scan(&hasReview, &hasInvestigation); err != nil {
			t.Fatalf("run %d: read final b comments: %v", run, err)
		}
		inp.hasReviewComment = hasReview > 0
		inp.hasInvestigationComment = hasInvestigation > 0
		expected := derivePipelineStage(&inp)

		finalStage := readPipelineStageInternal(t, ctx, b)
		if finalStage != expected {
			t.Fatalf("run %d: LWW race detected — b.pipeline_stage = %q, "+
				"want %q (re-derived from FINAL committed state). "+
				"A stale handler UPDATE clobbered a fresh derivation. "+
				"derivation inputs: status=%q impl_state=%q review_state=%q qa_state=%q pipeline_state=%q closed_at_not_null=%v has_review=%v has_investigation=%v",
				run, finalStage, expected,
				inp.status, inp.implState, inp.reviewState, inp.qaState, inp.pipelineState,
				inp.closedAtNotNull, inp.hasReviewComment, inp.hasInvestigationComment)
		}
		// Pin the expected final stage: rule 7 — review_state=approved,
		// qa_state=pending → Quality. If the mutator's UPDATE committed
		// (asserted above), the §5.7.1 derivation MUST be Quality.
		if finalStage != "Quality" {
			t.Fatalf("run %d: b.pipeline_stage = %q, want %q (rule 7 — review_state=approved, qa_state=pending)",
				run, finalStage, "Quality")
		}

		// c is in the same closure as b but has no review/investigation
		// comments and default state columns → rule 12 → Investigation.
		cStage := readPipelineStageInternal(t, ctx, c)
		if cStage != "Investigation" {
			t.Fatalf("run %d: c.pipeline_stage = %q, want %q (rule 12 — pending impl, no investigation comment)",
				run, cStage, "Investigation")
		}

		// Cleanup is handled by the fixture's t.Cleanup (org delete
		// cascades to items/edges/comments via FK ON DELETE CASCADE),
		// but each run creates fresh ids inside the same fixture so
		// the next iteration is independent.
	}
}
