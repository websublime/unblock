-- Require issued_to_user on every MCP API key (bead unblock-tv8.73).
-- Canonical DDL: docs/SPEC.md § 9.4.6 (post-migration state).
--
-- DECISION (Miguel, 2026-06-04): every MCP API key MUST be issued to a
-- user — there is no userless "org-level service key". A NULL-user key was
-- structurally unusable (auth.validateAPIKey would build an empty-UID
-- Identity that Encore's auth handler rejects). This migration removes the
-- broken NULL path at the schema level.
--
-- Pre-production: the table has no rows yet, so SET NOT NULL cannot fail and
-- no data migration is required (CLAUDE.md pre-prod stance, spec §3.3).
--
-- FK direction change: because a NOT NULL column cannot be SET NULL on
-- user-delete, the FK is swapped from ON DELETE SET NULL to ON DELETE
-- CASCADE — deleting a user deletes that user's API keys. Tool-call audit
-- history survives: mcp.tool_calls.api_key_id is ON DELETE SET NULL
-- (0070_mcp.up.sql), so audit rows are kept with api_key_id nulled.
--
-- The original FK was declared inline (column-level, unnamed) in
-- 0070_mcp.up.sql, so Postgres auto-named it `api_keys_issued_to_user_fkey`
-- (verified against the live local cluster). We drop that exact constraint
-- and re-add it with an explicit name and the new delete action.

ALTER TABLE mcp.api_keys
    ALTER COLUMN issued_to_user SET NOT NULL;

ALTER TABLE mcp.api_keys
    DROP CONSTRAINT api_keys_issued_to_user_fkey;

ALTER TABLE mcp.api_keys
    ADD CONSTRAINT api_keys_issued_to_user_fkey
        FOREIGN KEY (issued_to_user)
        REFERENCES auth.users(id)
        ON DELETE CASCADE;
