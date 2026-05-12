-- Bootstrap migration for the `unblock` database.
-- Installs the two Postgres extensions required by every other schema:
--   * pgcrypto — used by `auth.oauth_tokens.*_enc` (MEMORY_DEK encryption)
--   * pg_trgm  — used by trigram-backed indexes for ILIKE searches
-- See SPEC §3.2 (line 140) and OQ3 closure in §2 of
-- docs/specs/01-spec-backend-mvp.md.
--
-- Per SPEC §3.3 (no down.sql in P01) and §3.2 (the only file in this
-- migration set with `IF NOT EXISTS` clauses — extensions are idempotent;
-- subsequent migrations 0020..0090 assume a fresh schema).

CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE EXTENSION IF NOT EXISTS pg_trgm;
