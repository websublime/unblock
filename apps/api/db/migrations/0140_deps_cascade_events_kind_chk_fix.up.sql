-- Corrective forward migration: re-assert deps.cascade_events_kind_chk with
-- the full 4-kind set (bead unblock-tv8.79).
-- Canonical DDL: docs/specs/01-spec-backend-mvp.md § 3.2 (0140 row) and the
-- §9.4.4-mirroring cascade_events.kind enum block (both at 4 kinds).
--
-- ROOT CAUSE: round-6 cascade-symmetry (commit 3e0d00d) widened the CHECK on
-- deps.cascade_events from 2 kinds ('close','edge_removed') to 4
-- ('close','edge_added','edge_removed','state_change') by EDITING
-- 0050_deps.up.sql IN PLACE. golang-migrate (Encore's migration engine) keys
-- migrations by VERSION NUMBER, not content checksum, so any database that had
-- already applied 0050 before that edit never re-ran it and silently retains
-- the OLD 2-kind constraint. On such an environment the cascade subscriber's
-- audit INSERT (kind = msg.Reason) violates the stale CHECK with SQLSTATE
-- 23514 for the 'state_change' and 'edge_added' reasons, the handler errors,
-- and Encore Pub/Sub retries — producing a retry storm. The functional cascade
-- recompute (is_ready propagation) commits in its own tx before the audit
-- insert and is UNAFFECTED; only the audit row and the retry noise break.
--
-- FIX (roll-forward discipline, spec § 3.3): a NEW up-only forward migration —
-- 0050 is NOT re-edited. DROP CONSTRAINT IF EXISTS then ADD CONSTRAINT
-- re-asserts the constraint with the full 4-kind set under the SAME name
-- (cascade_events_kind_chk — named identifiers are contract, spec § 3.3).
-- The constraint name MUST stay identical: re-adding under a different name
-- would orphan the stale constraint on already-migrated environments.
--
-- Idempotent on both paths:
--   * Fresh DB (already 4-kind from the post-edit 0050): DROP+ADD yields an
--     identical constraint — a no-op-equivalent. The re-ADD re-validates
--     existing rows, but every cascade_events row carries one of the 4 kinds
--     BY CONSTRUCTION (the only writers are the cascade subscriber with
--     kind = msg.Reason gated to the 4 values, and the inline edge-removal
--     writer using 'edge_removed'), so validation cannot fail.
--   * Stale DB (2-kind): DROP+ADD corrects the drift so state_change /
--     edge_added audit inserts succeed.
--
-- golang-migrate wraps each up file in a single transaction, so the DROP+ADD
-- is atomic. Pre-production: no backfill or data migration is required.

ALTER TABLE deps.cascade_events
    DROP CONSTRAINT IF EXISTS cascade_events_kind_chk;

ALTER TABLE deps.cascade_events
    ADD CONSTRAINT cascade_events_kind_chk
        CHECK (kind IN ('close', 'edge_added', 'edge_removed', 'state_change'));
