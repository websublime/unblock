-- AR-14: audit every secret-sanitiser hit. Added in P02 (no P01 DDL).
-- One row per sanitiser match (a single remember may produce N rows).
-- Canonical DDL: docs/specs/02-spec-backend-complete.md §3.1(a) (bead unblock-8xb.7).
-- Forward-only: no .down.sql (the entire P01+P02 set is up-only — CG-13).
CREATE TABLE memory.sanitiser_events (
    id              text         PRIMARY KEY,                 -- ULID
    entry_id        text         REFERENCES memory.entries(id) ON DELETE CASCADE,
                                                              -- nullable: a periodic re-scan hit
                                                              -- (AR-14) or a sanitiser hit on a
                                                              -- providers payload records NULL here
    scope           text,                                    -- 'org'|'project'|'user' for memory hits; NULL for providers hits
    org_id          text         REFERENCES org.organizations(id) ON DELETE CASCADE,
    pattern_id      text         NOT NULL,                    -- stable id from the §5.4.1 registry, e.g. 'github_pat'
    category        text         NOT NULL,                    -- 'credential' | 'pii' (coarse class)
    source          text         NOT NULL,                    -- 'memory_write' | 'memory_rescan' | 'providers_payload'
    redaction_count integer      NOT NULL DEFAULT 1,          -- matches collapsed under one (pattern, field)
    detected_at     timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT sanitiser_events_category_chk
        CHECK (category IN ('credential', 'pii')),
    CONSTRAINT sanitiser_events_source_chk
        CHECK (source IN ('memory_write', 'memory_rescan', 'providers_payload'))
);
CREATE INDEX sanitiser_events_entry_idx ON memory.sanitiser_events (entry_id);
CREATE INDEX sanitiser_events_org_detected_idx
    ON memory.sanitiser_events (org_id, detected_at DESC);
CREATE INDEX sanitiser_events_pattern_idx ON memory.sanitiser_events (pattern_id);
