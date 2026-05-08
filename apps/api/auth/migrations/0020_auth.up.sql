-- Schema `auth` — identities, OAuth tokens, sessions.
-- Canonical DDL: docs/SPEC.md § 9.4.1.
-- See docs/specs/01-spec-backend-mvp.md § 3.2 / § 3.3 for migration rules.
-- Per § 3.3: no down.sql; CREATE SCHEMA IF NOT EXISTS is the only IF-NOT-EXISTS
-- usage permitted after 0010_bootstrap; subsequent CREATE TABLE / CREATE INDEX
-- statements assume a fresh schema.

CREATE SCHEMA IF NOT EXISTS auth;

-- Identities. Single primary identity per user (PRD FR-2).
CREATE TABLE auth.users (
    id                  text         PRIMARY KEY,                          -- ULID
    primary_provider    text         NOT NULL,                             -- 'github' | 'gitlab'
    primary_provider_id text         NOT NULL,                             -- provider-side user id
    email               text         NOT NULL,
    display_name        text         NOT NULL,
    avatar_url          text,
    created_at          timestamptz  NOT NULL DEFAULT now(),
    updated_at          timestamptz  NOT NULL DEFAULT now(),
    deleted_at          timestamptz,                                       -- soft delete
    CONSTRAINT users_primary_provider_chk
        CHECK (primary_provider IN ('github', 'gitlab')),
    CONSTRAINT users_primary_provider_unique
        UNIQUE (primary_provider, primary_provider_id)
);
CREATE UNIQUE INDEX users_email_active_uniq
    ON auth.users (lower(email))
    WHERE deleted_at IS NULL;

-- OAuth tokens (encrypted at rest). Linked to users; multiple providers per user
-- supported, but only one is primary per FR-2 (others are event-source attachments).
CREATE TABLE auth.oauth_tokens (
    id                text         PRIMARY KEY,                            -- ULID
    user_id           text         NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    provider          text         NOT NULL,                               -- 'github' | 'gitlab'
    access_token_enc  bytea        NOT NULL,                               -- pgp_sym_encrypt(...)
    refresh_token_enc bytea,                                               -- nullable; not all providers issue refresh tokens
    scopes            text[]       NOT NULL DEFAULT '{}',
    expires_at        timestamptz,
    created_at        timestamptz  NOT NULL DEFAULT now(),
    rotated_at        timestamptz,                                         -- last refresh
    CONSTRAINT oauth_tokens_provider_chk
        CHECK (provider IN ('github', 'gitlab')),
    CONSTRAINT oauth_tokens_user_provider_uniq
        UNIQUE (user_id, provider)
);
CREATE INDEX oauth_tokens_user_idx ON auth.oauth_tokens (user_id);

-- Sessions. HttpOnly cookie payload references the session id; rotation is
-- expressed as a new row + revocation of the previous one.
CREATE TABLE auth.sessions (
    id           text         PRIMARY KEY,                                 -- ULID; opaque session id
    user_id      text         NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    issued_at    timestamptz  NOT NULL DEFAULT now(),
    last_seen_at timestamptz  NOT NULL DEFAULT now(),
    expires_at   timestamptz  NOT NULL,
    revoked_at   timestamptz,
    user_agent   text,
    ip_inet      inet,
    CONSTRAINT sessions_expiry_chk
        CHECK (expires_at > issued_at)
);
-- Partial index: only active sessions matter for the auth hot path.
CREATE INDEX sessions_active_user_idx
    ON auth.sessions (user_id, last_seen_at DESC)
    WHERE revoked_at IS NULL;
