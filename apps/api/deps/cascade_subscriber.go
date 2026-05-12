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
	affected, err := bfsForwardBlocksClosure(ctx, msg.TriggeredByItemID)
	if err != nil {
		rlog.Error("deps: cascade subscriber BFS failed",
			"err", err, "reason", msg.Reason,
			"event_id", msg.EventID, "triggered_by", msg.TriggeredByItemID)
		return fmt.Errorf("deps: cascade BFS: %w", err)
	}

	// 2. Recompute pipeline_stage per §5.7.1 for every affected item.
	//    Idempotent UPDATE: WHERE pipeline_stage <> $new short-circuits
	//    a no-op write on re-delivery.
	if err := recomputePipelineStageForAffected(ctx, affected); err != nil {
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
// On depth overflow (cap hit at row 257+), the CTE silently truncates
// — Postgres's `WHERE r.depth < $N` terminates the recursion at the
// declared cap. The subscriber emits a Warn on truncation and proceeds
// with the bounded prefix (the cap is locked at 256 per RP01-3).
func bfsForwardBlocksClosure(ctx context.Context, seedID string) ([]string, error) {
	// Read up to cascadeBFSMaxDepth+1 rows so we can detect cap hits
	// — when the result count exceeds the cap, we know the walk
	// terminated on depth rather than exhaustion of the graph.
	rows, err := db.Query(ctx,
		`WITH RECURSIVE reachable(id, depth) AS (
		         SELECT $1::text, 0
		         UNION ALL
		         SELECT d.to_item, r.depth + 1
		           FROM deps.dependencies d
		           JOIN reachable r ON d.from_item = r.id
		          WHERE d.kind = 'blocks'
		            AND r.depth < $2
		       )
		       SELECT DISTINCT id FROM reachable
		       ORDER BY id`,
		seedID, cascadeBFSMaxDepth,
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
	if len(out) >= cascadeBFSMaxDepth {
		rlog.Warn("deps: cascade BFS hit depth cap",
			"seed", seedID, "cap", cascadeBFSMaxDepth, "collected", len(out))
	}
	return out, nil
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
// The is_ready column is NEVER written here. The lint analyzer
// (apps/api/shared/lint/no_direct_is_ready_write.go) enforces that
// every UPDATE in this file targets pipeline_stage only.
func recomputePipelineStageForAffected(ctx context.Context, affected []string) error {
	if len(affected) == 0 {
		return nil
	}

	// Fetch the four state columns + status + closed_at for every
	// affected item.
	rowMap := make(map[string]*itemDerivationInputs, len(affected))
	rows, err := db.Query(ctx,
		`SELECT id, status, pipeline_stage,
		        impl_state, review_state, qa_state, pipeline_state,
		        (closed_at IS NOT NULL)
		   FROM workitems.items
		  WHERE id = ANY($1::text[])`,
		affected,
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
	commentRows, err := db.Query(ctx,
		`SELECT item_id,
		        max(CASE WHEN kind = 'review'        THEN 1 ELSE 0 END) AS has_review,
		        max(CASE WHEN kind = 'investigation' THEN 1 ELSE 0 END) AS has_investigation
		   FROM workitems.comments
		  WHERE item_id = ANY($1::text[])
		  GROUP BY item_id`,
		affected,
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
	// Law 7's < 2s envelope.
	for _, id := range affected {
		inp, ok := rowMap[id]
		if !ok {
			// Item disappeared between BFS and derivation read
			// (FK ON DELETE CASCADE on org/project) — skip silently.
			continue
		}
		newStage := derivePipelineStage(inp)
		if newStage == inp.pipelineStage {
			// Idempotent no-op. Skip the UPDATE.
			continue
		}
		if _, err := db.Exec(ctx,
			`UPDATE workitems.items
			    SET pipeline_stage = $2
			  WHERE id = $1 AND pipeline_stage <> $2`,
			id, newStage,
		); err != nil {
			return fmt.Errorf("pipeline_stage update %s: %w", id, err)
		}
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
