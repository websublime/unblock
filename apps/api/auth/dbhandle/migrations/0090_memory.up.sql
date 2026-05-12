-- Schema `memory` — entries, entry_refs.
-- Canonical DDL: docs/SPEC.md § 9.4.8.
-- SCHEMA-ONLY in P01 (no service code in apps/api/memory/ until P02).
-- FK direction: memory -> auth, org (already migrated).
-- pg_trgm prerequisite for entries_key_trgm_idx is met by 0010_bootstrap.

CREATE SCHEMA IF NOT EXISTS memory;

-- Scoped memory entries (PRD § 6 / FR-13). Three scope kinds.
-- Value is encrypted at rest (NFR-7 sanitisation runs *before* encryption,
-- so the plaintext stored is already sanitised; the *_enc column contains
-- the sanitised plaintext encrypted with pgcrypto).
CREATE TABLE memory.entries (
    id          text         PRIMARY KEY,                                  -- ULID
    scope       text         NOT NULL,                                     -- 'org' | 'project' | 'user'
    org_id      text         REFERENCES org.organizations(id) ON DELETE CASCADE,
    project_id  text         REFERENCES org.projects(id) ON DELETE CASCADE,
    user_id     text         REFERENCES auth.users(id) ON DELETE CASCADE,
    author_id   text         REFERENCES auth.users(id) ON DELETE SET NULL,
    author_agent text,                                                     -- AgentKind if written by an agent
    key         text         NOT NULL,                                     -- short label / canonical fact name
    value_enc   bytea        NOT NULL,                                     -- pgp_sym_encrypt(sanitised_plaintext)
    value_size  integer      NOT NULL,                                     -- size of plaintext, bytes; ≤ 8192
    tags        text[]       NOT NULL DEFAULT '{}',
    -- tsvector built over the *plaintext* before encryption, stored alongside
    -- to enable indexed full-text search without decrypt-on-search. The
    -- ts_doc column holds *only* a tokenised, lowercased projection — not the
    -- full plaintext — so its leakage surface is bounded to indexable terms.
    ts_doc      tsvector     NOT NULL,
    created_at  timestamptz  NOT NULL DEFAULT now(),
    updated_at  timestamptz  NOT NULL DEFAULT now(),
    expires_at  timestamptz,
    CONSTRAINT entries_scope_chk
        CHECK (scope IN ('org', 'project', 'user')),
    -- Scope discriminator: exactly the right scope id is set.
    CONSTRAINT entries_scope_target_chk CHECK (
        (scope = 'org'     AND org_id IS NOT NULL AND project_id IS NULL  AND user_id IS NULL)
     OR (scope = 'project' AND project_id IS NOT NULL AND org_id IS NULL  AND user_id IS NULL)
     OR (scope = 'user'    AND user_id IS NOT NULL AND org_id IS NULL     AND project_id IS NULL)
    ),
    CONSTRAINT entries_size_chk CHECK (value_size > 0 AND value_size <= 8192),
    -- Uniqueness per (scope target, key) — partial unique indexes below
    CONSTRAINT entries_author_chk
        CHECK (author_id IS NOT NULL OR author_agent IS NOT NULL)
);
-- Per-scope uniqueness on key
CREATE UNIQUE INDEX entries_org_key_uniq
    ON memory.entries (org_id, key)
    WHERE scope = 'org';
CREATE UNIQUE INDEX entries_project_key_uniq
    ON memory.entries (project_id, key)
    WHERE scope = 'project';
CREATE UNIQUE INDEX entries_user_key_uniq
    ON memory.entries (user_id, key)
    WHERE scope = 'user';
-- FTS index
CREATE INDEX entries_ts_doc_gin_idx ON memory.entries USING gin (ts_doc);
-- Tag index (GIN on text[])
CREATE INDEX entries_tags_gin_idx   ON memory.entries USING gin (tags);
-- Trigram index on key for fuzzy lookups (uses pg_trgm extension)
CREATE INDEX entries_key_trgm_idx   ON memory.entries USING gin (key gin_trgm_ops);

-- Cross-references: a memory entry can reference work items, comments, PRs,
-- milestones, or be flagged as a general scope-level fact.
-- Modeled as a polymorphic junction with a kind discriminator.
CREATE TABLE memory.entry_refs (
    id         text         PRIMARY KEY,                                   -- ULID
    entry_id   text         NOT NULL REFERENCES memory.entries(id) ON DELETE CASCADE,
    ref_kind   text         NOT NULL,                                      -- 'workitem' | 'comment' | 'pr' | 'milestone' | 'general'
    ref_id     text         NOT NULL,                                      -- target id (no FK due to polymorphism;
                                                                           -- referential integrity by service layer).
                                                                           -- For ref_kind='general': ref_id holds the
                                                                           -- parent entry's scope_id — i.e., the memory
                                                                           -- is a general fact about its org/project/user
                                                                           -- scope, not pinned to any specific entity.
    created_at timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT entry_refs_kind_chk
        CHECK (ref_kind IN ('workitem', 'comment', 'pr', 'milestone', 'general')),
    CONSTRAINT entry_refs_unique UNIQUE (entry_id, ref_kind, ref_id)
);
CREATE INDEX entry_refs_entry_idx ON memory.entry_refs (entry_id);
CREATE INDEX entry_refs_target_idx ON memory.entry_refs (ref_kind, ref_id);
