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
		    )
		  WHERE id = $1
		  RETURNING is_ready`,
		itemID,
	).Scan(&newReady)
	if err != nil {
		return false, fmt.Errorf("deps: recomputeReady update: %w", err)
	}
	return newReady, nil
}
