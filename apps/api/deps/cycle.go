// cycle.go owns the §6.5 cycle-detection primitive used by AddEdge.
//
// The cycle CTE walks the forward 'blocks' closure starting from the
// proposed edge's to_item; if the proposed from_item appears in the
// reachable set, inserting the edge would close a cycle. The CTE uses
// the depth-counter pattern (WHERE depth < 256 in the recursive term)
// per research C5 — LIMIT inside the recursive term is undocumented
// PG behaviour and the depth counter is the standard guard.
//
// SPEC anchors:
//
//   - §6.5 (verbatim block) — the canonical CTE shape.
//   - §6.2 Tool 11 — cycle_path is mandatory on the error envelope.
//   - AR-8 — the 256 depth cap is a v1.0 product constraint.
//   - DDL 0050_deps.up.sql — deps.cycles is the forensic audit table.

package deps

import (
	"context"
	"errors"
	"fmt"

	"encore.dev/storage/sqldb"
)

// checkCycle runs the §6.5 depth-counter CTE against tx with the
// proposed edge endpoints. If the edge would close a cycle, returns
// the cycle path (ordered list of item ids: the walk from toItem
// forward to fromItem, with fromItem prepended to close the loop).
// Returns (nil, nil) when no cycle is detected.
//
// The CTE is extended from the literal §6.5 SELECT 1 form to project
// the walk path as a text[], so the caller can surface the path on
// the error envelope (acceptance criterion: data.cycle_path populated)
// and write deps.cycles forensics with a real path.
func checkCycle(ctx context.Context, tx *sqldb.Tx, fromItem, toItem string) ([]string, error) {
	var path []string
	err := tx.QueryRow(ctx,
		`WITH RECURSIVE reachable(id, depth, path) AS (
		    SELECT $2::text, 0, ARRAY[$2::text]
		    UNION ALL
		    SELECT d.to_item, r.depth + 1, r.path || d.to_item
		      FROM deps.dependencies d
		      JOIN reachable r ON d.from_item = r.id
		     WHERE d.kind = 'blocks'
		       AND r.depth < 256
		 )
		 SELECT $1::text || path
		   FROM reachable
		  WHERE id = $1
		  LIMIT 1`,
		fromItem, toItem,
	).Scan(&path)
	if err != nil {
		if errors.Is(err, sqldb.ErrNoRows) {
			return nil, nil
		}
		return nil, fmt.Errorf("deps: cycle CTE: %w", err)
	}
	return path, nil
}

// recordCycle writes a forensic row to deps.cycles after AddEdge
// rejects an edge for cycle violation. Called inside the same
// transaction as the cycle check; the transaction is rolled back by
// the caller after this row is INSERTed AND the audit row is committed
// in a SEPARATE follow-up transaction (the §6.5 transaction is rolled
// back to preserve the "no edge written on rejection" invariant, so
// the forensic INSERT cannot ride that rollback).
//
// rejectedBy may be empty (the caller could not resolve a user id from
// the auth context — e.g. a seeder run); the column accepts NULL.
func recordCycle(ctx context.Context, fromItem, toItem string, cyclePath []string, rejectedBy string) error {
	id, err := newULID()
	if err != nil {
		return err
	}
	var rejected *string
	if rejectedBy != "" {
		r := rejectedBy
		rejected = &r
	}
	if _, err := db.Exec(ctx,
		`INSERT INTO deps.cycles (id, from_item, to_item, cycle_path, rejected_by)
		 VALUES ($1, $2, $3, $4, $5)`,
		id, fromItem, toItem, cyclePath, rejected,
	); err != nil {
		return fmt.Errorf("deps: cycles insert: %w", err)
	}
	return nil
}
