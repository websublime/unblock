-- Schema `boards` — boards, columns.
-- Canonical DDL: docs/SPEC.md § 9.4.7.
-- SCHEMA-ONLY in P01 (no service code in apps/api/boards/ until P05).
-- FK direction: boards -> org, auth (already migrated).

CREATE SCHEMA IF NOT EXISTS boards;

CREATE TABLE boards.boards (
    id          text         PRIMARY KEY,                                  -- ULID
    org_id      text         NOT NULL REFERENCES org.organizations(id) ON DELETE CASCADE,
    project_id  text         REFERENCES org.projects(id) ON DELETE CASCADE,
    user_id     text         NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE, -- saved per user
    name        text         NOT NULL,
    description text,
    filters     jsonb        NOT NULL DEFAULT '{}'::jsonb,                 -- saved filter state (status, label, milestone, etc.)
    layout      text         NOT NULL DEFAULT 'kanban',                    -- 'kanban' | 'list' | 'graph' | 'roadmap'
    is_default  boolean      NOT NULL DEFAULT false,
    created_at  timestamptz  NOT NULL DEFAULT now(),
    updated_at  timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT boards_layout_chk
        CHECK (layout IN ('kanban', 'list', 'graph', 'roadmap'))
);
CREATE INDEX boards_org_user_idx ON boards.boards (org_id, user_id);
-- Only one default per (user, project) — partial unique.
CREATE UNIQUE INDEX boards_default_per_user_project_uniq
    ON boards.boards (user_id, COALESCE(project_id, ''))
    WHERE is_default = true;

-- Per-board column configuration (kanban). Columns are user-defined groupings;
-- each column has a filter (e.g. by status, by label).
CREATE TABLE boards.columns (
    id           text         PRIMARY KEY,                                 -- ULID
    board_id     text         NOT NULL REFERENCES boards.boards(id) ON DELETE CASCADE,
    name         text         NOT NULL,
    filter       jsonb        NOT NULL DEFAULT '{}'::jsonb,
    position     integer      NOT NULL,
    wip_limit    integer,                                                  -- nullable; null = unlimited
    color        text,
    created_at   timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT columns_position_chk CHECK (position >= 0),
    CONSTRAINT columns_wip_chk      CHECK (wip_limit IS NULL OR wip_limit > 0),
    CONSTRAINT columns_color_chk    CHECK (color IS NULL OR color ~ '^#[0-9a-fA-F]{6}$'),
    CONSTRAINT columns_board_position_uniq UNIQUE (board_id, position)
);
CREATE INDEX columns_board_idx ON boards.columns (board_id, position);
