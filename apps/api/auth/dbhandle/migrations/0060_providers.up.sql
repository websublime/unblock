-- Schema `providers` — installations, events, mappings.
-- Canonical DDL: docs/SPEC.md § 9.4.5.
-- SCHEMA-ONLY in P01 (no service code in apps/api/providers/ until P02).
-- FK direction: providers -> org, workitems, auth (already migrated).

CREATE SCHEMA IF NOT EXISTS providers;

CREATE TABLE providers.installations (
    id                  text         PRIMARY KEY,                          -- ULID
    org_id              text         NOT NULL REFERENCES org.organizations(id) ON DELETE CASCADE,
    project_id          text         REFERENCES org.projects(id) ON DELETE CASCADE,
    provider            text         NOT NULL,                             -- 'github' | 'gitlab'
    provider_account    text         NOT NULL,                             -- e.g. 'websublime'
    provider_repo       text,                                              -- nullable for org-level installs
    installation_id_enc bytea        NOT NULL,                             -- pgp_sym_encrypt(provider_installation_id)
    webhook_secret_enc  bytea        NOT NULL,                             -- per-install HMAC secret
    installed_by        text         REFERENCES auth.users(id) ON DELETE SET NULL,
    installed_at        timestamptz  NOT NULL DEFAULT now(),
    revoked_at          timestamptz,
    CONSTRAINT installations_provider_chk
        CHECK (provider IN ('github', 'gitlab')),
    CONSTRAINT installations_target_uniq
        UNIQUE (provider, provider_account, provider_repo)
);
CREATE INDEX installations_org_idx ON providers.installations (org_id);

-- Webhook events. Audit + dedup. Idempotency on (provider, delivery_id).
CREATE TABLE providers.events (
    id              text         PRIMARY KEY,                              -- ULID (our id, not provider's)
    installation_id text         NOT NULL REFERENCES providers.installations(id) ON DELETE CASCADE,
    provider        text         NOT NULL,                                 -- denormalised for hot lookup
    delivery_id     text         NOT NULL,                                 -- e.g. X-GitHub-Delivery
    event_type      text         NOT NULL,                                 -- e.g. 'issues.opened'
    payload         jsonb        NOT NULL,
    signature_ok    boolean      NOT NULL,
    received_at     timestamptz  NOT NULL DEFAULT now(),
    processed_at    timestamptz,
    error           text,
    CONSTRAINT events_provider_chk
        CHECK (provider IN ('github', 'gitlab')),
    CONSTRAINT events_delivery_uniq
        UNIQUE (provider, delivery_id)
);
CREATE INDEX events_installation_received_idx
    ON providers.events (installation_id, received_at DESC);
CREATE INDEX events_unprocessed_idx
    ON providers.events (received_at)
    WHERE processed_at IS NULL;
CREATE INDEX events_payload_gin_idx ON providers.events USING gin (payload);

-- PII retention policy on providers.events.payload (referenced from AR-13/14):
-- raw webhook payloads can carry user emails, repo metadata, and OAuth-related
-- fields. The retention policy is:
--   * Raw payload retained 90 days from `received_at`.
--   * After 90 days a scheduled job (Encore cron, runs daily) replaces the
--     payload with a metadata-only digest:
--         { "event_type": <string>, "actor_login": <hash>, "repo": <hash>,
--           "delivery_id": <string>, "digest_at": <ts> }
--     The digest preserves the audit's debugging value (we still know which
--     event type came from which installation when) without retaining
--     identifying free-text payload fields.
--   * Email addresses and any matched credential patterns are redacted
--     **on insert** by a per-row sanitiser running before the row is
--     committed. The 90-day truncation is the second layer; the sanitiser
--     is the first.
-- The exact digest schema and the redactor pattern set land in the P02 spec.
-- A test asserts that a payload older than 90 days has been digested.

-- Provider ↔ work item mapping. Bidirectional sync key.
CREATE TABLE providers.mappings (
    id              text         PRIMARY KEY,                              -- ULID
    installation_id text         NOT NULL REFERENCES providers.installations(id) ON DELETE CASCADE,
    item_id         text         NOT NULL REFERENCES workitems.items(id) ON DELETE CASCADE,
    provider        text         NOT NULL,
    provider_kind   text         NOT NULL,                                 -- 'issue' | 'pull_request'
    provider_id     text         NOT NULL,                                 -- provider-side id (string for portability)
    provider_url    text         NOT NULL,
    last_synced_at  timestamptz,
    drift_detected_at timestamptz,
    CONSTRAINT mappings_provider_chk
        CHECK (provider IN ('github', 'gitlab')),
    CONSTRAINT mappings_kind_chk
        CHECK (provider_kind IN ('issue', 'pull_request')),
    CONSTRAINT mappings_external_uniq
        UNIQUE (provider, provider_kind, provider_id),
    CONSTRAINT mappings_item_provider_uniq
        UNIQUE (item_id, provider, provider_kind)
);
CREATE INDEX mappings_item_idx          ON providers.mappings (item_id);
CREATE INDEX mappings_installation_idx  ON providers.mappings (installation_id);
CREATE INDEX mappings_drift_idx
    ON providers.mappings (drift_detected_at)
    WHERE drift_detected_at IS NOT NULL;
