-- Migration 0100 — extend items_ready_partial_idx to cover the §6.2 Tool 2
-- (ready) ordering: (priority asc, created_at asc, id asc).
--
-- The original index in 0040_workitems.up.sql:144-146 covered only
-- (org_id, project_id, priority) and forced the planner into a heap
-- sort for the spec-mandated tiebreakers. Per the orchestrator's
-- DECISION on bead unblock-tv8.17 (D-2, 2026-05-14, decision #2), the
-- index gets extended HERE so the `ready` MCP tool's hot path stays
-- on a pure index scan against items_ready_partial_idx without an
-- extra sort node. Pre-prod stance + zero production data → plain
-- DROP + CREATE in a single transaction (no CREATE INDEX CONCURRENTLY
-- required). Predicate is preserved verbatim from 0040.
--
-- SPEC anchor: docs/specs/01-spec-backend-mvp.md § 6.2 Tool 2 (lines
-- 1187-1198) + § 11.2 NFR-1 (p99 < 2 s on prime → ready → claim).

DROP INDEX IF EXISTS workitems.items_ready_partial_idx;

CREATE INDEX items_ready_partial_idx
    ON workitems.items (org_id, project_id, priority, created_at, id)
    WHERE is_ready = true AND status = 'Ready' AND closed_at IS NULL;
