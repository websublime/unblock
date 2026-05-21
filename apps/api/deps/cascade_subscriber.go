// cascade_subscriber.go owns the Pub/Sub subscription against
// CascadeRequestedTopic and the §6.3.2 subscriber algorithm. The
// subscriber is the SOLE writer of workitems.items.pipeline_stage
// (Regime B per SPEC §6.3.0 line 1699-1705 and §11.3 bullet (a)).
//
// Regime split (SPEC §6.3.0):
//
//   - Regime A — is_ready (single-hop, writer-inline). is_ready is
//     recomputed by the call site that mutated the row/edge inside the
//     same transaction as the mutation, via deps.recomputeReady (or
//     deps.RecomputeReadyForBlocksDownstream for workitems.Close). The
//     subscriber MUST NOT write is_ready (SPEC §6.3.0 line 1688 +
//     §6.3.2 lines 1819-1820 + §11.3 bullet (b)).
//   - Regime B — pipeline_stage (multi-hop, subscriber-only). Every
//     call site that materially mutates §5.7.1 derivation inputs
//     publishes CascadeRequested with a matching Reason. The
//     subscriber walks the forward 'blocks' closure from
//     TriggeredByItemID and recomputes pipeline_stage for every item
//     in the closure per the §5.7.1 derivation table.
//
// Idempotency (AR-11 / SPEC §6.3.2):
//
//   - The audit row INSERT uses ON CONFLICT (event_id, triggered_by_item_id)
//     DO NOTHING. Redeliveries of the same logical event collapse to
//     a no-op on the second insert. The (event_id, triggered_by_item_id)
//     UNIQUE constraint on deps.cascade_events is the structural
//     idempotency key.
//   - The pipeline_stage UPDATE is value-equality idempotent: the
//     WHERE clause checks `pipeline_stage <> $new`, so a re-run on a
//     stable graph writes nothing.
//   - For Reason='edge_removed', deps.RemoveEdge writes the inline
//     audit row with an event_id captured BEFORE BEGIN; the
//     post-commit publish reuses that event_id. The subscriber's
//     INSERT below collapses to no-op via the same ON CONFLICT clause
//     (round-6 tension #1 — exactly one audit row per logical remove).
//     The pipeline_stage recompute pass still runs.
//
// BFS depth cap (AR-8 / RP01-3): 256. On overflow the subscriber logs
// a warning and returns nil — a partial cascade is preferable to a
// retry loop that would replay the same overflow.
//
// Trace correlation (SPEC §10.2 Option B): CascadeRequested carries
// TraceID as a typed payload field because Encore Pub/Sub does not
// propagate context.Context across the topic boundary. The subscriber
// writes msg.TraceID back to deps.cascade_events.trace_id (NULL when
// empty — non-MCP code paths may publish without an originating trace).
//
// CascadeCompleted publish is best-effort. A publish failure logs a
// warning and is NOT returned as an error — the source-of-truth audit
// row is the deps.cascade_events INSERT above, which has already
// committed.

package deps

import (
	"context"
	"fmt"
	"time"

	"encore.app/shared/ulid"
	"encore.dev/pubsub"
	"encore.dev/rlog"
)

// cascadeBFSMaxDepth caps the forward 'blocks' BFS depth at AR-8 (256).
// Matches closureMaxDepth in deps.go but lives here to keep the
// subscriber file self-contained.
const cascadeBFSMaxDepth = 256

// _ is the Encore Pub/Sub subscription wiring. The blank identifier
// matches the SPEC's literal block at §6.3.2 line 1770 — Encore wires
// the handler on import of this package; no other reference is needed.
//
// Subscription name `deps-cascade-subscriber` is the kebab-case SPEC
// literal (§6.3.2 line 1770). Encore v1.52.1 requires the name argument
// to be a Go string literal (parser error E1184), so the name lives
// inline at the call site rather than as a named constant.
//
// SubscriptionConfig leaves retry policy unset: Encore default backoff
// applies (SPEC §6.3.2 line 1773-1774). At-least-once delivery is
// inherited from the topic's TopicConfig in cascade.go.
//
//nolint:gochecknoglobals // pubsub subscriptions are package-level by Encore contract.
var _ = pubsub.NewSubscription(
	CascadeRequestedTopic,
	"deps-cascade-subscriber",
	pubsub.SubscriptionConfig[*CascadeRequested]{
		Handler: handleCascadeRequested,
	},
)

// handleCascadeRequested is the §6.3.2 subscriber body. Walks the
// forward 'blocks' closure from msg.TriggeredByItemID (max depth 256),
// recomputes pipeline_stage per §5.7.1 on every affected item, writes
// one deps.cascade_events row with kind=msg.Reason, and publishes
// CascadeCompleted (best-effort).
//
// Reason dispatch: the four documented kinds share the same body
// (forward BFS + per-item §5.7.1 derivation + audit row). The switch
// exists for an explicit defensive `default` branch that drops
// unknown Reason values rather than crashing — the publisher set is
// closed by spec but a malformed redelivery from a future code path
// should never crash the subscriber.
//
// The handler returns nil on every path except an unrecoverable DB
// error. A return error triggers Encore's retry — desirable for
// transient connection failures, undesirable for permanent errors
// (e.g. a row that no longer exists). We treat "BFS seed row gone"
// as a non-error (the triggering item was deleted between publish and
// subscribe) so the message is not retried forever.
func handleCascadeRequested(ctx context.Context, msg *CascadeRequested) error {
	if msg == nil {
		rlog.Warn("deps: cascade subscriber received nil message")
		return nil
	}

	switch msg.Reason {
	case "close", "edge_added", "edge_removed", "state_change":
		// Documented kinds — fall through to the shared body below.
	default:
		// Defensive: drop unknown Reason values. The publisher set is
		// closed by §6.3.0 but a malformed redelivery should not
		// crash the subscriber.
		rlog.Warn("deps: cascade subscriber received unknown Reason",
			"reason", msg.Reason, "event_id", msg.EventID,
			"triggered_by", msg.TriggeredByItemID)
		return nil
	}

	if msg.TriggeredByItemID == "" {
		rlog.Warn("deps: cascade subscriber received empty TriggeredByItemID",
			"reason", msg.Reason, "event_id", msg.EventID)
		return nil
	}

	// 1. BFS forward along 'blocks' edges from TriggeredByItemID,
	//    collecting affected ids. The seed itself is INCLUDED so its
	//    own pipeline_stage is recomputed (a status='Done' flip can
	//    move the triggered item's pipeline_stage to 'Done').
	//
	//    Tenant defence-in-depth (unblock-tv8.50): the BFS is gated on
	//    msg.OrgID and msg.ProjectID via a JOIN against workitems.items
	//    in both the anchor and the recursive step. Cross-tenant edges
	//    are already rejected upstream by deps.AddEdge (deps.go §6.5),
	//    so in practice the walk stays within one tenant; the JOIN
	//    enforces the property structurally so a future regression in
	//    the writer path or a direct DDL bypass cannot leak a cascade
	//    across orgs/projects.
	affected, err := bfsForwardBlocksClosure(ctx, msg.TriggeredByItemID, msg.OrgID, msg.ProjectID)
	if err != nil {
		rlog.Error("deps: cascade subscriber BFS failed",
			"err", err, "reason", msg.Reason,
			"event_id", msg.EventID, "triggered_by", msg.TriggeredByItemID)
		return fmt.Errorf("deps: cascade BFS: %w", err)
	}

	// 2. Recompute pipeline_stage per §5.7.1 for every affected item.
	//    Idempotent UPDATE: WHERE pipeline_stage <> $new short-circuits
	//    a no-op write on re-delivery. The derivation reads are also
	//    tenant-gated (unblock-tv8.50) — symmetric defence-in-depth so a
	//    tampered `affected` set (e.g. a tenant-bypassing id injected
	//    between BFS and derivation read) cannot pull another org's row.
	if err := recomputePipelineStageForAffected(ctx, affected, msg.OrgID, msg.ProjectID, msg.EventID); err != nil {
		rlog.Error("deps: cascade subscriber pipeline_stage recompute failed",
			"err", err, "reason", msg.Reason,
			"event_id", msg.EventID, "triggered_by", msg.TriggeredByItemID)
		return fmt.Errorf("deps: cascade pipeline_stage: %w", err)
	}

	// 3. Audit row insert (ON CONFLICT DO NOTHING). For edge_removed
	//    this collapses with the inline row written by deps.RemoveEdge
	//    (tension #1 — exactly one row per logical remove).
	if err := insertCascadeEventRow(ctx, msg, affected); err != nil {
		rlog.Error("deps: cascade subscriber audit insert failed",
			"err", err, "reason", msg.Reason,
			"event_id", msg.EventID, "triggered_by", msg.TriggeredByItemID)
		return fmt.Errorf("deps: cascade audit insert: %w", err)
	}

	// 4. Publish CascadeCompleted (best-effort). The audit row above is
	//    the source of truth — a publish failure here is logged and
	//    swallowed.
	if _, err := CascadeCompletedTopic.Publish(ctx, &CascadeCompleted{
		EventID:           msg.EventID,
		TriggeredByItemID: msg.TriggeredByItemID,
		AffectedItemIDs:   affected,
		CascadedCount:     len(affected),
		CompletedAt:       time.Now().UTC(),
	}); err != nil {
		rlog.Warn("deps: cascade subscriber CascadeCompleted publish failed (audit committed)",
			"err", err, "event_id", msg.EventID,
			"triggered_by", msg.TriggeredByItemID)
	}
	return nil
}

// bfsForwardBlocksClosure walks the forward 'blocks' closure from
// seedID via a recursive CTE — same shape as closureSQL("outgoing")
// in deps.go but INCLUDES the seed and is depth-capped at AR-8 (256).
//
// Why include the seed: §5.7.1 derivation depends on the seed's own
// state columns (e.g. status='Done' after workitems.Close), so the
// seed's pipeline_stage must be re-derived too. closureSQL excludes
// the seed because its callers (Closure RPC) want neighbours-only;
// the cascade pass wants both.
//
// On depth overflow (depth 256+ would be reached), the CTE silently
// truncates — Postgres's `WHERE r.depth < $N` terminates the recursion
// at the declared cap. The subscriber emits a Warn on truncation and
// proceeds with the bounded prefix (the cap is locked at 256 per
// RP01-3). The warning predicate compares the observed MAX(depth) from
// the recursive CTE against cascadeBFSMaxDepth-1 — NOT len(out), which
// conflates closure SIZE with closure DEPTH (unblock-tv8.49).
//
// Tenant predicate (unblock-tv8.50 / defence-in-depth): both the
// anchor and the recursive step JOIN workitems.items and gate on
// (i.org_id = orgID AND ($projectID = ” OR i.project_id = projectID)).
// Cross-tenant rows in deps.dependencies are impossible today because
// deps.AddEdge (§6.5) rejects them upstream, but a future writer-path
// regression or a direct DDL bypass would otherwise leak a cascade
// across orgs/projects via the recursive walk. The JOIN closes that
// gap structurally — its cost is one items_pk index probe per edge
// row, well inside the AR-8 (256) depth cap and Law 7's < 2s envelope.
//
// The deps.dependencies schema has no org_id / project_id columns
// (see apps/api/db/migrations/0050_deps.up.sql), so tenant filtering
// MUST be expressed via JOIN against workitems.items. The same
// shape is used by deps.recomputeReady (recompute.go:50-69).
//
// projectID may be empty: workitems.items.project_id is nullable
// (org-scoped items are permitted by §9.4.2). The `$N = ” OR
// project_id = $N` shape mirrors deps.go:767-768 (CascadeEvents read
// path) — when projectID is empty the predicate degrades to org_id
// alone, when non-empty it narrows to that project.
//
// If the publisher's (orgID, projectID) disagrees with the seed's
// actual row, the anchor SELECT returns no rows, the recursive step
// has no seed to walk from, and the function returns an empty slice.
// The audit row INSERT in insertCascadeEventRow still writes with
// msg.OrgID as authoritative — the audit captures what was REQUESTED,
// not what was REACHABLE. This is intentional: the publisher's claim
// must remain visible in the audit even when the walk yields nothing.
func bfsForwardBlocksClosure(ctx context.Context, seedID, orgID, projectID string) ([]string, error) {
	// The recursive CTE is referenced by two top-level SELECTs so the
	// depth-cap diagnostic compares the actual MAX(depth) observed in
	// the walk against the cap — NOT len(out), which conflates closure
	// SIZE with closure DEPTH (a wide-but-shallow graph would otherwise
	// emit a false 'BFS hit depth cap' warning per unblock-tv8.49).
	//
	// The first SELECT returns the DISTINCT id list (unchanged contract
	// — public signature still returns []string and the audit row's
	// affected_item_ids depends on it). The second SELECT extracts the
	// scalar MAX(depth); COALESCE(max(depth), -1) maps an empty reachable
	// set (e.g. tenant mismatch — see header comment) to a sentinel that
	// the warning predicate treats as 'no overflow'.
	rows, err := db.Query(ctx,
		`WITH RECURSIVE reachable(id, depth) AS (
		         SELECT i.id, 0
		           FROM workitems.items i
		          WHERE i.id = $1
		            AND i.org_id = $3
		            AND ($4 = '' OR i.project_id = $4)
		         UNION ALL
		         SELECT d.to_item, r.depth + 1
		           FROM deps.dependencies d
		           JOIN reachable r ON d.from_item = r.id
		           JOIN workitems.items i ON i.id = d.to_item
		          WHERE d.kind = 'blocks'
		            AND r.depth < $2
		            AND i.org_id = $3
		            AND ($4 = '' OR i.project_id = $4)
		       )
		       SELECT DISTINCT id FROM reachable
		       ORDER BY id`,
		seedID, cascadeBFSMaxDepth, orgID, projectID,
	)
	if err != nil {
		return nil, fmt.Errorf("bfs query: %w", err)
	}
	defer rows.Close()

	out := make([]string, 0, 8)
	for rows.Next() {
		var id string
		if err := rows.Scan(&id); err != nil {
			return nil, fmt.Errorf("bfs scan: %w", err)
		}
		out = append(out, id)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("bfs iter: %w", err)
	}

	// Second round-trip: extract MAX(depth) from the same recursive
	// definition. This is bounded by AR-8 (cascadeBFSMaxDepth=256) so
	// the extra trip is negligible vs Law 7's < 2s envelope (see
	// DECISION on unblock-tv8.49). COALESCE → -1 when reachable is
	// empty (tenant mismatch or seed absent) so the scanner never
	// faces a NULL.
	var maxDepth int
	if err := db.QueryRow(ctx,
		`WITH RECURSIVE reachable(id, depth) AS (
		         SELECT i.id, 0
		           FROM workitems.items i
		          WHERE i.id = $1
		            AND i.org_id = $3
		            AND ($4 = '' OR i.project_id = $4)
		         UNION ALL
		         SELECT d.to_item, r.depth + 1
		           FROM deps.dependencies d
		           JOIN reachable r ON d.from_item = r.id
		           JOIN workitems.items i ON i.id = d.to_item
		          WHERE d.kind = 'blocks'
		            AND r.depth < $2
		            AND i.org_id = $3
		            AND ($4 = '' OR i.project_id = $4)
		       )
		       SELECT COALESCE(MAX(depth), -1) FROM reachable`,
		seedID, cascadeBFSMaxDepth, orgID, projectID,
	).Scan(&maxDepth); err != nil {
		return nil, fmt.Errorf("bfs max-depth: %w", err)
	}

	if shouldEmitDepthCapWarning(maxDepth) {
		rlog.Warn("deps: cascade BFS hit depth cap",
			"seed", seedID, "cap", cascadeBFSMaxDepth,
			"max_depth", maxDepth, "collected", len(out))
	}
	return out, nil
}

// shouldEmitDepthCapWarning returns true when the observed maximum
// depth from bfsForwardBlocksClosure's recursive CTE indicates the
// walk terminated on the AR-8 depth cap rather than on graph
// exhaustion.
//
// CTE invariant: `WHERE r.depth < cascadeBFSMaxDepth` admits depth
// values 0..(cascadeBFSMaxDepth-1). When the cap fires, the highest
// depth collected in `reachable` is exactly cascadeBFSMaxDepth-1
// (=255). Strictly lower observed depths mean the recursion ran out
// of edges before hitting the cap.
//
// Sentinel: -1 (from COALESCE(max(depth), -1) on an empty reachable
// set — e.g. tenant mismatch on the anchor SELECT) is treated as
// 'no overflow', matching the case where the BFS returns no rows.
//
// Pure function — extracted for unit testability (the rlog warning
// itself has no test-capture harness). See unblock-tv8.49.
func shouldEmitDepthCapWarning(maxDepth int) bool {
	return maxDepth >= cascadeBFSMaxDepth-1
}

// itemDerivationInputs captures the fields the §5.7.1 derivation table
// reads from workitems.items and the existence predicates evaluated
// against workitems.comments.
type itemDerivationInputs struct {
	id              string
	status          string
	pipelineStage   string // current value — used to skip idempotent UPDATEs
	implState       string
	reviewState     string
	qaState         string
	pipelineState   string
	closedAtNotNull bool

	hasReviewComment        bool
	hasInvestigationComment bool
}

// recomputePipelineStageForAffected runs the §5.7.1 derivation for
// every affected item and writes pipeline_stage where it differs from
// the current value.
//
// Issues ONE batched SELECT against workitems.comments per cascade pass
// per SPEC §5.7.1 line 781-787: a single GROUP BY scan over the
// affected set is bounded by len(affected) rather than N+1.
//
// The UPDATE is per-item — Postgres has no clean "update many distinct
// values" path without unnest+VALUES, and the affected set in P01 is
// bounded at 256. The per-item UPDATE includes the value-equality
// guard so repeated delivery converges to the same row state.
//
// Concurrency / LWW race fix (unblock-tv8.51): the items SELECT, the
// comments SELECT, and the per-item UPDATE pass all run inside a single
// short transaction. The items SELECT acquires `FOR NO KEY UPDATE`
// row locks with a deterministic `ORDER BY id` so two concurrent
// subscriber invocations on overlapping closures SERIALISE rather than
// interleave a read-derive-write race:
//
//   - Without the tx + row lock, handler A could SELECT state S_A,
//     derive stage A in Go memory, and meanwhile handler B (publishing
//     from a state-column write that committed between A's SELECT and
//     A's UPDATE) reads the newer state S_B and derives stage B.
//     If B's UPDATE commits first, A's UPDATE then clobbers B with the
//     stale A derivation. The pre-existing `WHERE pipeline_stage <> $2`
//     value-equality guard only short-circuits the no-op case; it does
//     not close this race.
//   - With FOR NO KEY UPDATE + ORDER BY id, handler B blocks on the
//     row lock until A commits, then re-reads the fresh state and
//     derives correctly. ORDER BY id (lexicographic) guarantees the
//     same lock-acquisition order across handlers, eliminating the
//     classic deadlock-on-overlapping-sets pathology.
//   - FOR NO KEY UPDATE (rather than plain FOR UPDATE) is the softer
//     lock that suffices here: we mutate only pipeline_stage which is
//     not part of any unique key, so blocking FK references to this
//     row (e.g. workitems.comments inserts) is unnecessary. Same
//     concurrency guarantee, narrower contention footprint.
//   - The audit INSERT (insertCascadeEventRow) stays OUTSIDE this tx —
//     its idempotency mechanism is the (event_id, triggered_by_item_id)
//     UNIQUE constraint, not the tx, and bundling it would extend the
//     row-lock window without changing correctness.
//   - The forward BFS (bfsForwardBlocksClosure) also stays outside —
//     it is a read-only topology walk and including it would
//     unnecessarily extend the lock hold time.
//
// Encore Pub/Sub MaxConcurrency cannot be relied upon as a serialiser:
// encore.dev v1.52.1 documents that the setting has no effect on
// Encore Cloud environments, so concurrent handler dispatch across
// instances is possible on the target deploy platform. The DB-side
// fix above is the only correct closure.
//
// Tenant predicate (unblock-tv8.50 / defence-in-depth): both reads are
// gated by (org_id = orgID AND ($projectID = ” OR project_id =
// projectID)). Today the BFS already filters the input set by the same
// predicate, so the predicate here is redundant in the happy path —
// but it is essential for symmetric defence-in-depth: a tampered
// `affected` slice (a tenant-bypassing id injected between BFS and
// derivation read by future code, or a debug-path call that bypasses
// the BFS) cannot pull another org's row. workitems.comments has no
// org_id column, so the predicate is enforced via a sub-SELECT against
// workitems.items keyed on item_id. The per-item UPDATE WHERE clause
// adds the same predicate so a write cannot escape the publisher's
// tenant either.
//
// The is_ready column is NEVER written here. The lint analyzer
// (apps/api/shared/lint/no_direct_is_ready_write.go) enforces that
// every UPDATE in this file targets pipeline_stage only.
func recomputePipelineStageForAffected(ctx context.Context, affected []string, orgID, projectID, eventID string) error {
	if len(affected) == 0 {
		return nil
	}

	// Open the short transaction that frames the items SELECT, the
	// comments SELECT, and the per-item UPDATE pass. See the function
	// doc comment for the LWW-race rationale (unblock-tv8.51).
	//
	// Tx-lifecycle rlog (unblock-tv8.51 review hardening): log
	// Begin/Commit failures locally for finer-grained ops visibility.
	// The caller (handleCascadeRequested) also wraps the returned
	// error with rlog.Error, but the local log carries the tx_phase
	// field so ops dashboards can split begin-vs-commit failure rates
	// (commit failures usually indicate row-lock contention or
	// serialisation conflicts, begin failures indicate pool starvation
	// or connection issues — different remediation paths).
	tx, err := db.Begin(ctx)
	if err != nil {
		rlog.Error("deps: cascade pipeline_stage tx failed",
			"err", err, "event_id", eventID,
			"org_id", orgID, "tx_phase", "begin")
		return fmt.Errorf("pipeline_stage tx begin: %w", err)
	}
	defer func() { _ = tx.Rollback() }()

	// Fetch the four state columns + status + closed_at for every
	// affected item. Tenant-gated read (unblock-tv8.50). Row-locked
	// `FOR NO KEY UPDATE` with deterministic `ORDER BY id` to serialise
	// concurrent subscriber invocations on overlapping closures
	// (unblock-tv8.51).
	rowMap := make(map[string]*itemDerivationInputs, len(affected))
	rows, err := tx.Query(ctx,
		`SELECT id, status, pipeline_stage,
		        impl_state, review_state, qa_state, pipeline_state,
		        (closed_at IS NOT NULL)
		   FROM workitems.items
		  WHERE id = ANY($1::text[])
		    AND org_id = $2
		    AND ($3 = '' OR project_id = $3)
		  ORDER BY id
		  FOR NO KEY UPDATE`,
		affected, orgID, projectID,
	)
	if err != nil {
		return fmt.Errorf("affected state read: %w", err)
	}
	for rows.Next() {
		inp := &itemDerivationInputs{}
		if err := rows.Scan(
			&inp.id, &inp.status, &inp.pipelineStage,
			&inp.implState, &inp.reviewState, &inp.qaState, &inp.pipelineState,
			&inp.closedAtNotNull,
		); err != nil {
			rows.Close()
			return fmt.Errorf("affected state scan: %w", err)
		}
		rowMap[inp.id] = inp
	}
	if err := rows.Err(); err != nil {
		rows.Close()
		return fmt.Errorf("affected state iter: %w", err)
	}
	rows.Close()

	// Batched comment existence predicate (SPEC §5.7.1 line 781-787).
	// workitems.comments has no org_id column — gate via sub-SELECT
	// against workitems.items keyed on item_id (unblock-tv8.50). Read
	// inside the same tx as the locked items SELECT so the derivation
	// inputs come from one consistent snapshot (unblock-tv8.51).
	commentRows, err := tx.Query(ctx,
		`SELECT c.item_id,
		        max(CASE WHEN c.kind = 'review'        THEN 1 ELSE 0 END) AS has_review,
		        max(CASE WHEN c.kind = 'investigation' THEN 1 ELSE 0 END) AS has_investigation
		   FROM workitems.comments c
		   JOIN workitems.items i ON i.id = c.item_id
		  WHERE c.item_id = ANY($1::text[])
		    AND i.org_id = $2
		    AND ($3 = '' OR i.project_id = $3)
		  GROUP BY c.item_id`,
		affected, orgID, projectID,
	)
	if err != nil {
		return fmt.Errorf("affected comments read: %w", err)
	}
	for commentRows.Next() {
		var itemID string
		var hasReview, hasInvestigation int
		if err := commentRows.Scan(&itemID, &hasReview, &hasInvestigation); err != nil {
			commentRows.Close()
			return fmt.Errorf("affected comments scan: %w", err)
		}
		if inp, ok := rowMap[itemID]; ok {
			inp.hasReviewComment = hasReview > 0
			inp.hasInvestigationComment = hasInvestigation > 0
		}
	}
	if err := commentRows.Err(); err != nil {
		commentRows.Close()
		return fmt.Errorf("affected comments iter: %w", err)
	}
	commentRows.Close()

	// Per-item derive + idempotent UPDATE. Affected set is bounded at
	// AR-8 (256) so the per-row UPDATE is O(N) bounded — well inside
	// Law 7's < 2s envelope. Writes go through tx (the same lock-
	// holding transaction as the items SELECT above) so the
	// read-derive-update sequence is atomic per cascade pass.
	for _, id := range affected {
		inp, ok := rowMap[id]
		if !ok {
			// Item not present in the tenant-filtered derivation read.
			// Two reasons this can happen:
			//   (a) the row was deleted between BFS and derivation
			//       read (FK ON DELETE CASCADE on org/project); the
			//       BFS predicate would also drop it on a fresh walk,
			//       but the BFS already committed its snapshot.
			//   (b) (post unblock-tv8.50) the row's tenant disagrees
			//       with msg.OrgID / msg.ProjectID. This is now also
			//       blocked at the BFS layer for happy-path cascades,
			//       but a future caller that bypasses the BFS and
			//       feeds this function a tampered slice still cannot
			//       reach a cross-tenant row.
			// Either way: skip silently. No write occurs.
			continue
		}
		newStage := derivePipelineStage(inp)
		if newStage == inp.pipelineStage {
			// Idempotent no-op. Skip the UPDATE.
			continue
		}
		// Tenant-gated UPDATE (unblock-tv8.50): the WHERE clause
		// repeats the org_id / project_id predicate so a write cannot
		// escape the publisher's tenant even if the rowMap lookup
		// above were ever defeated by a future refactor.
		res, err := tx.Exec(ctx,
			`UPDATE workitems.items
			    SET pipeline_stage = $2
			  WHERE id = $1
			    AND pipeline_stage <> $2
			    AND org_id = $3
			    AND ($4 = '' OR project_id = $4)`,
			id, newStage, orgID, projectID,
		)
		if err != nil {
			return fmt.Errorf("pipeline_stage update %s: %w", id, err)
		}
		// Defence-in-depth surfacing (unblock-tv8.50): the rowMap entry
		// existed (passed the `!ok` guard above) and the in-memory
		// derivation produced a stage that differs from the row's
		// current value (passed the `newStage == inp.pipelineStage`
		// guard above). The only way the tenant-gated UPDATE can write
		// zero rows is if the row's org_id / project_id disagree with
		// the publisher's claim — i.e. a future bypass-caller fed this
		// function a mismatched (orgID, projectID) against an item that
		// the BFS tenant predicate would also have dropped. Warn so the
		// regression is visible instead of silently skipped.
		if res.RowsAffected() == 0 {
			rlog.Warn(
				"pipeline_stage UPDATE no-op on tenant-mismatched item — possible cross-tenant publisher regression",
				"item_id", id,
				"org_id", orgID,
				"project_id", projectID,
				"event_id", eventID,
			)
		}
	}

	if err := tx.Commit(); err != nil {
		rlog.Error("deps: cascade pipeline_stage tx failed",
			"err", err, "event_id", eventID,
			"org_id", orgID, "tx_phase", "commit")
		return fmt.Errorf("pipeline_stage tx commit: %w", err)
	}
	return nil
}

// derivePipelineStage applies the §5.7.1 derivation table. First match
// wins. Returns one of the six §6.1 PipelineStage values: Investigation,
// Implementation, Review, Quality, Deferred, Done. The DDL CHECK
// constraint enforces the same set (items_pipeline_stage_chk).
//
// Pure function — no DB I/O. Unit-testable from a leaf test file
// (deps_unit_test.go) without the Encore runtime. Note that this leaf
// test cannot live inside encore.app/deps because plain `go test` on
// this package panics at package init (cascade.go's pubsub.NewTopic
// declaration). The unit test runs in a `deps_test` external package
// under `encore test` instead.
//
// Order is locked verbatim from SPEC docs/SPEC.md §5.7.1 lines 766-779.
// Do not reorder without a spec amendment.
func derivePipelineStage(inp *itemDerivationInputs) string {
	// Rule 1+2: pipeline_state = needs_human OR paused → Deferred.
	if inp.pipelineState == "needs_human" || inp.pipelineState == "paused" {
		return "Deferred"
	}
	// Rule 3: pipeline_state = no_investigation AND impl_state = pending → Implementation.
	if inp.pipelineState == "no_investigation" && inp.implState == "pending" {
		return "Implementation"
	}
	// Rule 4: status = Done OR (qa_state = passed AND closed_at IS NOT NULL) → Done.
	if inp.status == "Done" || (inp.qaState == "passed" && inp.closedAtNotNull) {
		return "Done"
	}
	// Rule 5: qa_state = passed → Quality. (closure pending)
	if inp.qaState == "passed" {
		return "Quality"
	}
	// Rule 6: qa_state = failed → Quality.
	if inp.qaState == "failed" {
		return "Quality"
	}
	// Rule 7: review_state = approved AND qa_state = pending → Quality.
	if inp.reviewState == "approved" && inp.qaState == "pending" {
		return "Quality"
	}
	// Rule 8: review_state = needs_rework → Implementation.
	if inp.reviewState == "needs_rework" {
		return "Implementation"
	}
	// Rule 9: impl_state = done AND review_state = pending AND has review-kind comment → Review.
	if inp.implState == "done" && inp.reviewState == "pending" && inp.hasReviewComment {
		return "Review"
	}
	// Rule 10: impl_state = done AND review_state = pending AND no review comment → Implementation.
	if inp.implState == "done" && inp.reviewState == "pending" && !inp.hasReviewComment {
		return "Implementation"
	}
	// Rule 11: impl_state = pending AND has investigation-kind comment → Implementation.
	if inp.implState == "pending" && inp.hasInvestigationComment {
		return "Implementation"
	}
	// Rule 12: impl_state = pending AND no investigation comment → Investigation.
	return "Investigation"
}

// insertCascadeEventRow writes one deps.cascade_events row with
// kind=msg.Reason and the affected set, idempotent on
// (event_id, triggered_by_item_id).
//
// Nullability: project_id is nullable on workitems.items (the org may
// have org-scoped items with project_id IS NULL); we mirror that here
// by writing NULL when msg.ProjectID is empty. trace_id is also nullable;
// we write NULL when msg.TraceID is empty (non-MCP publishers — e.g.
// admin scripts — may omit it).
//
// The audit row id is minted fresh per call. The (event_id,
// triggered_by_item_id) UNIQUE constraint is the idempotency mechanism;
// the row id is only ever the PRIMARY KEY for the audit table itself
// and is irrelevant to dedup.
func insertCascadeEventRow(ctx context.Context, msg *CascadeRequested, affected []string) error {
	rowID, err := ulid.New()
	if err != nil {
		return fmt.Errorf("audit row ulid: %w", err)
	}

	// Nullable projection. Encore's sqldb passes a nil *string as
	// SQL NULL; an empty string would violate the FK semantics for
	// project_id (the column references org.projects(id)).
	var projectColumn *string
	if msg.ProjectID != "" {
		p := msg.ProjectID
		projectColumn = &p
	}
	var traceColumn *string
	if msg.TraceID != "" {
		t := msg.TraceID
		traceColumn = &t
	}

	// affected_item_ids is NOT NULL DEFAULT '{}' in the DDL — pass an
	// empty (non-nil) slice when affected is empty so pgx writes the
	// empty array literal rather than NULL.
	if affected == nil {
		affected = []string{}
	}

	if _, err := db.Exec(ctx,
		`INSERT INTO deps.cascade_events
		   (id, event_id, kind, org_id, project_id,
		    triggered_by_item_id, affected_item_ids, cascaded_count, trace_id)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
		 ON CONFLICT (event_id, triggered_by_item_id) DO NOTHING`,
		rowID, msg.EventID, msg.Reason, msg.OrgID, projectColumn,
		msg.TriggeredByItemID, affected, len(affected), traceColumn,
	); err != nil {
		return fmt.Errorf("audit insert: %w", err)
	}
	return nil
}
