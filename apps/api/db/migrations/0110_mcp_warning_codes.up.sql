-- Migration 0110 — additive mcp.tool_calls.warning_codes column for the
-- §7.1 success-side warnings audit channel (bead unblock-tv8.63).
--
-- The §7.1 success-side warnings of a tool result (currently only
-- set_state's intent_comment_dropped on the best-effort AppendComment
-- failure path) are audited via this jsonb array column: it stores the
-- `code` strings present on the tool's success-result warnings[], or the
-- empty array `[]` when none. jsonb (not text) so it is a true 0..N list,
-- queryable for the FR-9 quality analytics
-- (WHERE warning_codes @> '["intent_comment_dropped"]', GIN-indexable —
-- mirroring the existing arguments jsonb + tool_calls_arguments_gin_idx
-- precedent) and array-normalised on write.
--
-- result_kind STAYS 'ok' on the partial-failure path (the call
-- succeeded); the tool_calls_result_chk CHECK constraint
-- (0070_mcp.up.sql:70-71) is intentionally UNTOUCHED — the audit widening
-- is this warning column, NOT a new result_kind value.
--
-- NEW sequential migration (NOT an amend of 0070): apps/api/db/migrations/
-- is append-only and 0070 already shipped + ran (local + CI clusters at
-- schema_migrations.version = 100). golang-migrate (Encore's runner) only
-- applies migrations numbered HIGHER than the current max, so this file
-- continues the established multiples-of-10 sequence (0010..0100 -> 0110).
-- Up-only — no 0110_mcp_warning_codes.down.sql, per the §3.3 "No down.sql
-- files in P01" convention (zero .down.sql files exist under apps/api).
--
-- SPEC anchor: docs/specs/01-spec-backend-mvp.md § 8.1.1 (warning_codes
-- audit column) + § 7.1 (success-side warnings) + § 6.2 Tool 13
-- (intent_comment partial-failure). NOTE: §8.1.1 prose pins the filename
-- as 0071; that numbering is below the already-applied version 100 and
-- would never run — implemented as 0110 (DEVIATION logged on
-- unblock-tv8.63 for the reviewer to back-fold the §8.1.1 filename).

ALTER TABLE mcp.tool_calls
    ADD COLUMN warning_codes jsonb NOT NULL DEFAULT '[]'::jsonb;

CREATE INDEX tool_calls_warning_codes_gin_idx
    ON mcp.tool_calls USING gin (warning_codes);
