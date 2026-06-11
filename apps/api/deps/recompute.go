// recompute.go owns the shared inline helper that recomputes
// workitems.items.is_ready for a single item (Regime A, single-hop).
//
// Round-6 §6.3.0 splits the cascade subsystem into two writer regimes:
//
//   - Regime A — is_ready (single-hop, writer-inline). Every call site
//     that mutates a row or edge in a way that can flip is_ready for
//     the DIRECTLY affected item recomputes is_ready synchronously in
//     the same SQL transaction as the mutation, via this helper.
//   - Regime B — pipeline_stage (multi-hop, subscriber-only). The
//     cascade subscriber is the sole writer of pipeline_stage and runs
//     on CascadeRequested deliveries.
//
// recomputeReady is the single source of truth for the §6.5 closure
// CTE that derives is_ready: "the item is ready iff no incoming
// 'blocks' edge originates from a non-Done item." Called by:
//
//   - deps.AddEdge (Tool 11 / §6.5): after INSERT, recompute for to_item.
//   - deps.RemoveEdge (Tool 12 / §6.5): after DELETE, recompute for to_item.
//   - The cascade subscriber (C-3 / §6.3.2) is FORBIDDEN from writing
//     is_ready — it only reads pipeline_stage. The single-hop neighbours
//     of a closed item are recomputed inline by workitems.Close, NOT
//     by this helper from the subscriber path.
//
// The helper is unexported on purpose: Encore //encore:api endpoints
// cannot accept *sqldb.Tx parameters, so this stays a plain Go function
// inside the deps package.

package deps

import (
	"context"
	"fmt"

	"encore.dev/storage/sqldb"
)

// recomputeReady recomputes is_ready for itemID inside the supplied
// transaction and writes the new value. Returns the new is_ready value.
//
// Implements the §6.5 closure CTE: an item is ready iff no incoming
// 'blocks' edge originates from a non-Done item. The UPDATE is
// idempotent — if is_ready already matches the computed value, the
// row is rewritten with the same value (no observable change).
//
// Status reconciliation (round-16, bead unblock-tv8.71, §6.6 transition
// map). In the SAME statement the helper reconciles the item's Status
// enum against the recomputed is_ready so the two never drift:
//
//   - Ready → Blocked DEMOTION: when is_ready recomputes to false AND the
//     item is currently 'Ready' (unclaimed), status flips to 'Blocked'.
//     This is the inverse of promote (Tool 15): a new unmet incoming
//     blocks edge added to a Ready item demotes it. An 'InProgress'
//     (claimed) item is NEVER demoted — §6.6 keeps it InProgress and only
//     flips is_ready=false (the claimant resolves the blocker). A
//     'Backlog' item likewise keeps its status; only is_ready changes.
//   - Blocked → Ready RECOVERY: when is_ready recomputes to true AND the
//     item is currently 'Blocked', status flips back to 'Ready'. This
//     fires when the last open blocker closes (workitems.Close downstream
//     recompute) or its edge is removed (RemoveEdge). A claimed item is
//     never Blocked (see §6.6), so there is no claim to retain here.
//
// Backlog items with is_ready=true remain 'Backlog' (promote is an
// explicit agent action — §6.6); no status transition is implied by
// readiness alone outside the Ready⇄Blocked pair above.
//
// This is the SOLE write path for workitems.items.is_ready inside the
// deps package (and the only one outside workitems.Close's inline
// neighbour recompute). The shared/lint/no_direct_is_ready_write
// analyzer allow-lists encore.app/deps for exactly this UPDATE.
func recomputeReady(ctx context.Context, tx *sqldb.Tx, itemID string) (bool, error) {
	var newReady bool
	err := tx.QueryRow(ctx,
		`UPDATE workitems.items
		    SET is_ready = (
		      NOT EXISTS (
		        SELECT 1 FROM deps.dependencies d2
		          JOIN workitems.items i ON i.id = d2.from_item
		         WHERE d2.to_item = $1 AND d2.kind = 'blocks' AND i.status <> 'Done'
		      )
		    ),
		    status = CASE
		      WHEN status = 'Ready' AND EXISTS (
		        SELECT 1 FROM deps.dependencies d3
		          JOIN workitems.items i3 ON i3.id = d3.from_item
		         WHERE d3.to_item = $1 AND d3.kind = 'blocks' AND i3.status <> 'Done'
		      ) THEN 'Blocked'
		      WHEN status = 'Blocked' AND NOT EXISTS (
		        SELECT 1 FROM deps.dependencies d4
		          JOIN workitems.items i4 ON i4.id = d4.from_item
		         WHERE d4.to_item = $1 AND d4.kind = 'blocks' AND i4.status <> 'Done'
		      ) THEN 'Ready'
		      ELSE status
		    END,
		    updated_at = now()
		  WHERE id = $1
		  RETURNING is_ready`,
		itemID,
	).Scan(&newReady)
	if err != nil {
		return false, fmt.Errorf("deps: recomputeReady update: %w", err)
	}
	return newReady, nil
}

// RecomputeReadyForBlocksDownstream is the exported Regime A helper for
// the workitems.Close (Tool 6) call site. It recomputes is_ready for
// every direct 'blocks' downstream neighbour of fromItemID inside the
// supplied transaction and returns the subset of those neighbours that
// flipped to is_ready=true as a result.
//
// SPEC §6.3.0 line 1691-1692 mandates workitems.Close inline-recompute
// is_ready for the closed item's direct blocks neighbours. The lint
// allow-list at apps/api/shared/lint/no_direct_is_ready_write.go gates
// is_ready UPDATEs to encore.app/deps — so workitems.Close cannot inline
// the UPDATE itself and must call this exported helper instead. The
// helper accepts a *sqldb.Tx so workitems.Close can run the recompute
// in the SAME transaction as the status='Done' write (Regime A
// invariant: the writer's transaction holds the readiness flip).
//
// Direction: forward along outgoing 'blocks' edges. A row
// `(from_item=fromItemID, to_item=X, kind='blocks')` in deps.dependencies
// means "fromItemID blocks X". When fromItemID flips to status='Done',
// X may now satisfy the NOT EXISTS closure (§6.5) and become ready —
// hence we recompute X. We do NOT walk multi-hop here; that is Regime B
// (pipeline_stage only) and runs in the cascade subscriber.
//
// Returns the list of neighbour ids whose is_ready value was true AFTER
// the recompute (i.e. items that are now ready). An empty list is a
// valid result — the closed item may have had no 'blocks' downstream,
// or none of the downstream items became ready (other blockers remain).
// The caller logs the result via rlog and never fails on emptiness.
//
// Idempotency: the underlying recomputeReady UPDATE is value-equality
// idempotent. Calling this helper twice in the same transaction for
// the same fromItemID returns the same list (no double-flip).
func RecomputeReadyForBlocksDownstream(ctx context.Context, tx *sqldb.Tx, fromItemID string) ([]string, error) {
	if fromItemID == "" {
		return nil, fmt.Errorf("deps: RecomputeReadyForBlocksDownstream: empty fromItemID")
	}
	rows, err := tx.Query(ctx,
		`SELECT to_item FROM deps.dependencies
		  WHERE from_item = $1 AND kind = 'blocks'`,
		fromItemID,
	)
	if err != nil {
		return nil, fmt.Errorf("deps: RecomputeReadyForBlocksDownstream select: %w", err)
	}
	neighbours := make([]string, 0)
	for rows.Next() {
		var id string
		if err := rows.Scan(&id); err != nil {
			rows.Close()
			return nil, fmt.Errorf("deps: RecomputeReadyForBlocksDownstream scan: %w", err)
		}
		neighbours = append(neighbours, id)
	}
	if err := rows.Err(); err != nil {
		rows.Close()
		return nil, fmt.Errorf("deps: RecomputeReadyForBlocksDownstream iter: %w", err)
	}
	rows.Close()

	flipped := make([]string, 0, len(neighbours))
	for _, id := range neighbours {
		ready, err := recomputeReady(ctx, tx, id)
		if err != nil {
			return nil, fmt.Errorf("deps: RecomputeReadyForBlocksDownstream recompute %s: %w", id, err)
		}
		if ready {
			flipped = append(flipped, id)
		}
	}
	return flipped, nil
}
