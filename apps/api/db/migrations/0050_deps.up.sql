-- Schema `deps` — dependencies, cycles, cascade_events.
-- Canonical DDL: docs/SPEC.md § 9.4.4.
-- FK direction: deps -> workitems, org, auth (already migrated).
-- cascade_events.kind discriminator ('close' | 'edge_removed') ships here
-- per docs/specs/01-spec-backend-mvp.md § 6.3.2 (round-2 review).

CREATE SCHEMA IF NOT EXISTS deps;

-- Edges between work items. "from blocks to" semantics: from must be Done
-- before to can become Ready.
CREATE TABLE deps.dependencies (
    id          text         PRIMARY KEY,                                  -- ULID
    from_item   text         NOT NULL REFERENCES workitems.items(id) ON DELETE CASCADE,
    to_item     text         NOT NULL REFERENCES workitems.items(id) ON DELETE CASCADE,
    kind        text         NOT NULL DEFAULT 'blocks',                    -- 'blocks' | 'related' (related not enforced for ready)
    created_at  timestamptz  NOT NULL DEFAULT now(),
    created_by  text         REFERENCES auth.users(id) ON DELETE SET NULL,
    CONSTRAINT dependencies_no_self_loop_chk
        CHECK (from_item <> to_item),
    CONSTRAINT dependencies_kind_chk
        CHECK (kind IN ('blocks', 'related')),
    CONSTRAINT dependencies_pair_uniq
        UNIQUE (from_item, to_item, kind)
);
CREATE INDEX dependencies_from_idx ON deps.dependencies (from_item);
CREATE INDEX dependencies_to_idx   ON deps.dependencies (to_item);
-- Partial index: the cycle-detection CTE only walks 'blocks' edges.
CREATE INDEX dependencies_blocks_to_idx
    ON deps.dependencies (to_item, from_item)
    WHERE kind = 'blocks';

-- Cycle audit. When the cycle-prevention CTE rejects a write, we record the
-- attempted edge for forensics. The CTE itself is in SPEC § 9.4.9.
CREATE TABLE deps.cycles (
    id           text         PRIMARY KEY,                                 -- ULID
    detected_at  timestamptz  NOT NULL DEFAULT now(),
    from_item    text         NOT NULL,                                    -- not FK (the row may not exist)
    to_item      text         NOT NULL,
    cycle_path   text[]       NOT NULL,                                    -- ordered list of item ids forming the cycle
    rejected_by  text         REFERENCES auth.users(id) ON DELETE SET NULL
);
CREATE INDEX cycles_detected_idx ON deps.cycles (detected_at DESC);

-- Cascade events audit (PRD M-5 — "cascade events per day"). Every successful
-- run of the cascade subscriber (Law 1) writes one row here. The M-5 metric
-- query aggregates this table grouped by (org_id, date_trunc('day', triggered_at));
-- this decouples the metric from the observability stack so the number is
-- reproducible from Postgres alone, even after retention windows on traces
-- have rolled over. The table is also used as a forensic record when a
-- cascade affects a surprising set of items.
CREATE TABLE deps.cascade_events (
    id                    text         PRIMARY KEY,                       -- ULID (audit row id)
    -- Publisher-generated event id (ULID) carried as a typed field on the
    -- Pub/Sub message payload. Encore Go's subscriber handler signature does
    -- NOT expose envelope metadata (research C1), so the publisher embeds
    -- this id at emit time and the subscriber reads it from the payload.
    -- The (event_id, triggered_by_item_id) UNIQUE constraint below is the
    -- structural mitigation for at-least-once redelivery (AR-11).
    event_id              text         NOT NULL,
    -- Cascade kind discriminator. Added in round-2 review iterations:
    --   'close'        — written by the cascade subscriber when a close
    --                    event (Tool 6 / workitems.Close) arrives via
    --                    Pub/Sub; walks the forward 'blocks' closure
    --                    (multi-hop, possibly large affected set).
    --   'edge_removed' — written INLINE by Tool 12 (remove_dependency)
    --                    in the same SQL transaction as DELETE FROM
    --                    deps.dependencies; single-hop only (the direct
    --                    to_item is the only candidate to flip ready).
    -- Future kinds (e.g. 'state_change' for Pub/Sub-driven state-cascade
    -- writes deferred to P02+) extend the CHECK constraint additively in
    -- their own phase migration.
    kind                  text         NOT NULL,
    org_id                text         NOT NULL REFERENCES org.organizations(id) ON DELETE CASCADE,
    project_id            text         REFERENCES org.projects(id) ON DELETE SET NULL,
    triggered_by_item_id  text         REFERENCES workitems.items(id) ON DELETE SET NULL,
    -- The full set of items whose `is_ready` flipped to true as a result.
    -- Stored as text[] (item ULIDs). For M-5 the cardinality matters more
    -- than the membership; cardinality is denormalised in `cascaded_count`
    -- so the metric query never needs to inspect the array.
    affected_item_ids     text[]       NOT NULL DEFAULT '{}',
    cascaded_count        integer      NOT NULL DEFAULT 0,
    triggered_at          timestamptz  NOT NULL DEFAULT now(),
    trace_id              text,                                            -- correlates with mcp.tool_calls.trace_id
    CONSTRAINT cascade_events_kind_chk
        CHECK (kind IN ('close', 'edge_removed')),
    CONSTRAINT cascade_events_count_chk
        CHECK (cascaded_count >= 0),
    -- AR-11 idempotency key. A redelivered Pub/Sub message carries the same
    -- payload bytes (including event_id), so the second insert is rejected
    -- by this constraint and the subscriber's UPDATE pass is a no-op on a
    -- stable graph.
    CONSTRAINT cascade_events_event_trigger_uniq
        UNIQUE (event_id, triggered_by_item_id)
);
-- Hot-path index for the M-5 metric query (per-org, by day).
CREATE INDEX cascade_events_org_triggered_idx
    ON deps.cascade_events (org_id, triggered_at DESC);
CREATE INDEX cascade_events_project_idx
    ON deps.cascade_events (project_id, triggered_at DESC)
    WHERE project_id IS NOT NULL;
-- Partial index: cascades that actually moved the graph (cascaded_count > 0).
-- M-5's "non-zero on the median active org" target reads through this index.
CREATE INDEX cascade_events_nonzero_idx
    ON deps.cascade_events (org_id, triggered_at DESC)
    WHERE cascaded_count > 0;
