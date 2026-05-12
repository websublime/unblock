-- Schema `workitems` — milestones, items, labels, item_labels, comments.
-- Canonical DDL: docs/SPEC.md § 9.4.3.
-- FK direction: workitems -> org, auth (already migrated in 0020/0030).
-- FTS DDL (AF1): per-table tsvector GENERATED ... STORED + GIN index inline
-- below — items_fts_idx, comments_fts_idx (PG GIN cannot span tables; the
-- search RPC uses UNION ALL across both indexes per docs/specs/01 § 3.4).

CREATE SCHEMA IF NOT EXISTS workitems;

-- Milestones are recursive (PRD § 6.3). Self-FK + scope FKs.
CREATE TABLE workitems.milestones (
    id                  text         PRIMARY KEY,                          -- ULID
    parent_milestone_id text         REFERENCES workitems.milestones(id) ON DELETE SET NULL,
    org_id              text         REFERENCES org.organizations(id) ON DELETE CASCADE,
    project_id          text         REFERENCES org.projects(id) ON DELETE CASCADE,
    name                text         NOT NULL,
    description         text,
    start_date          date         NOT NULL,
    end_date            date         NOT NULL,
    cancelled_at        timestamptz,
    cancelled_reason    text,
    created_at          timestamptz  NOT NULL DEFAULT now(),
    updated_at          timestamptz  NOT NULL DEFAULT now(),
    -- M-INV-1: no self-loop
    CONSTRAINT milestones_no_self_loop_chk
        CHECK (parent_milestone_id IS NULL OR parent_milestone_id <> id),
    -- Scope is XOR: org-wide OR project-local, never both, never neither.
    CONSTRAINT milestones_scope_xor_chk
        CHECK ((org_id IS NOT NULL AND project_id IS NULL)
            OR (org_id IS NULL AND project_id IS NOT NULL)),
    -- Date sanity
    CONSTRAINT milestones_date_range_chk
        CHECK (end_date >= start_date)
);
CREATE INDEX milestones_parent_idx       ON workitems.milestones (parent_milestone_id);
CREATE INDEX milestones_org_idx          ON workitems.milestones (org_id);
CREATE INDEX milestones_project_idx      ON workitems.milestones (project_id);
CREATE INDEX milestones_active_idx
    ON workitems.milestones (project_id, start_date)
    WHERE cancelled_at IS NULL;
-- Note: M-INV-2 (cycle prevention), M-INV-3 (range containment), M-INV-5 (scope match),
-- M-INV-6 (max depth = 4), M-INV-7 (item-milestone scope reachability) are enforced
-- in app code via recursive CTE checks at insert/update time. See SPEC § 9.4.9.

-- Work items. The single most-touched table in the product.
CREATE TABLE workitems.items (
    id                   text         PRIMARY KEY,                         -- ULID
    org_id               text         NOT NULL REFERENCES org.organizations(id) ON DELETE CASCADE,
    project_id           text         REFERENCES org.projects(id) ON DELETE CASCADE,
    milestone_id         text         REFERENCES workitems.milestones(id) ON DELETE SET NULL,
    parent_id            text         REFERENCES workitems.items(id) ON DELETE SET NULL,
    discovered_from_id   text         REFERENCES workitems.items(id) ON DELETE SET NULL,
    type                 text         NOT NULL DEFAULT 'task',             -- 'epic' | 'task' | 'finding'
    title                text         NOT NULL,
    body                 text         NOT NULL DEFAULT '',
    -- § 6.1 enums
    status               text         NOT NULL DEFAULT 'Backlog',          -- 'Backlog' | 'Ready' | 'InProgress' | 'Blocked' | 'Done'
    priority             text         NOT NULL DEFAULT 'P3',               -- 'P0'..'P4'
    pipeline_stage       text         NOT NULL DEFAULT 'Investigation',    -- § 6.1 PipelineStage values; SUBSCRIBER-MAINTAINED — derived from impl/review/qa/pipeline_state per § 5.7.1; do not write directly outside the cascade subscriber
    agent_kind           text,                                             -- § 6.1 AgentKind values; nullable until claim
    -- § 6.2 three orthogonal dimensions
    impl_state           text         NOT NULL DEFAULT 'pending',          -- 'pending' | 'done'
    review_state         text         NOT NULL DEFAULT 'pending',          -- 'pending' | 'approved' | 'needs_rework'
    qa_state             text         NOT NULL DEFAULT 'pending',          -- 'pending' | 'passed' | 'failed'
    pipeline_state       text         NOT NULL DEFAULT 'running',          -- 'running' | 'needs_human' | 'paused' | 'no_investigation'
    -- § 6.6 finding fields
    severity             text,                                             -- only meaningful when type='finding'
    kind_of_finding      text,                                             -- 'review' | 'qa'; only when type='finding'
    -- Claim
    claimed_by_id        text         REFERENCES auth.users(id) ON DELETE SET NULL,
    claimed_by_agent     text,                                             -- AgentKind value of the claimer
    claimed_at           timestamptz,
    -- Cascade-materialised readiness (NOT a generated column; updated by deps subscriber)
    is_ready             boolean      NOT NULL DEFAULT false,
    -- Milestone audit (collapsed from the deleted item_milestone junction)
    milestone_assigned_at timestamptz,
    milestone_assigned_by text         REFERENCES auth.users(id) ON DELETE SET NULL,
    -- Lifecycle
    created_at           timestamptz  NOT NULL DEFAULT now(),
    updated_at           timestamptz  NOT NULL DEFAULT now(),
    closed_at            timestamptz,
    -- Constraints
    CONSTRAINT items_no_self_parent_chk
        CHECK (parent_id IS NULL OR parent_id <> id),
    CONSTRAINT items_no_self_discovery_chk
        CHECK (discovered_from_id IS NULL OR discovered_from_id <> id),
    CONSTRAINT items_type_chk
        CHECK (type IN ('epic', 'task', 'finding')),
    CONSTRAINT items_status_chk
        CHECK (status IN ('Backlog', 'Ready', 'InProgress', 'Blocked', 'Done')),
    CONSTRAINT items_priority_chk
        CHECK (priority IN ('P0', 'P1', 'P2', 'P3', 'P4')),
    CONSTRAINT items_pipeline_stage_chk
        CHECK (pipeline_stage IN ('Investigation', 'Implementation', 'Review', 'Quality', 'Deferred', 'Done')),
    CONSTRAINT items_agent_kind_chk
        CHECK (agent_kind IS NULL OR agent_kind IN ('claude-code', 'copilot', 'cursor', 'codex', 'aider', 'custom')),
    CONSTRAINT items_impl_state_chk
        CHECK (impl_state IN ('pending', 'done')),
    CONSTRAINT items_review_state_chk
        CHECK (review_state IN ('pending', 'approved', 'needs_rework')),
    CONSTRAINT items_qa_state_chk
        CHECK (qa_state IN ('pending', 'passed', 'failed')),
    CONSTRAINT items_pipeline_state_chk
        CHECK (pipeline_state IN ('running', 'needs_human', 'paused', 'no_investigation')),
    CONSTRAINT items_severity_chk
        CHECK (severity IS NULL
            OR severity IN ('critical', 'major', 'minor', 'risk', 'extra', 'deviation')),
    CONSTRAINT items_kind_of_finding_chk
        CHECK (kind_of_finding IS NULL OR kind_of_finding IN ('review', 'qa')),
    -- Findings must declare severity + originating bead + a parent epic.
    -- PRD § 6.6 promises findings live "under the parent epic" — parent_id
    -- is required, and the service layer asserts the parent's type='epic'
    -- (deferred check; not expressible as a single-row CHECK).
    CONSTRAINT items_finding_required_fields_chk
        CHECK (
            (type <> 'finding')
            OR (severity IS NOT NULL
                AND kind_of_finding IS NOT NULL
                AND discovered_from_id IS NOT NULL
                AND parent_id IS NOT NULL)
        ),
    -- A claim implies status InProgress or Done. The (claimed_by_id NULL,
    -- status Done) combination is also legal: closing an item via the
    -- override path or via cascade promotion may finalise an item that was
    -- never claimed. Conversely, once claimed, an item's claim audit is
    -- preserved through close — `claimed_by_id` and `claimed_at` are NOT
    -- nulled on close. (Research AF3: the asymmetry is intentional; close
    -- preserves the claim history so the audit trail of who completed the
    -- work survives indefinitely.)
    CONSTRAINT items_claim_status_chk
        CHECK (
            (claimed_by_id IS NULL AND claimed_at IS NULL)
            OR (claimed_by_id IS NOT NULL AND claimed_at IS NOT NULL AND status IN ('InProgress', 'Done'))
        )
);
-- Hot-path indexes
CREATE INDEX items_org_status_idx        ON workitems.items (org_id, status);
CREATE INDEX items_project_status_idx    ON workitems.items (project_id, status);
CREATE INDEX items_milestone_idx         ON workitems.items (milestone_id);
CREATE INDEX items_parent_idx            ON workitems.items (parent_id);
CREATE INDEX items_discovered_from_idx   ON workitems.items (discovered_from_id);
CREATE INDEX items_claimed_by_idx        ON workitems.items (claimed_by_id);
-- Partial index: the `ready` MCP tool's hot path. p99 < 2 s depends on this.
CREATE INDEX items_ready_partial_idx
    ON workitems.items (org_id, project_id, priority)
    WHERE is_ready = true AND status = 'Ready' AND closed_at IS NULL;
-- Full-text search backing the `search` MCP tool (research AF1). Generated
-- column over title + body; GIN-indexed. Multi-table search at query time
-- uses UNION ALL across this index and `comments_fts_idx` (PG GIN cannot
-- span two tables); per-row trigram fallback covers fuzzy match.
ALTER TABLE workitems.items ADD COLUMN fts tsvector
    GENERATED ALWAYS AS (
        setweight(to_tsvector('english', coalesce(title, '')), 'A') ||
        setweight(to_tsvector('english', coalesce(body,  '')), 'B')
    ) STORED;
CREATE INDEX items_fts_idx ON workitems.items USING GIN (fts);
-- Partial index: in-progress board view.
CREATE INDEX items_in_progress_idx
    ON workitems.items (org_id, project_id, claimed_at DESC)
    WHERE status = 'InProgress';

-- User-facing labels (PRD § 6.4). Scope is org XOR project.
CREATE TABLE workitems.labels (
    id          text         PRIMARY KEY,                                  -- ULID
    org_id      text         REFERENCES org.organizations(id) ON DELETE CASCADE,
    project_id  text         REFERENCES org.projects(id) ON DELETE CASCADE,
    name        text         NOT NULL,
    color       text         NOT NULL,                                     -- hex, '#rrggbb'
    description text,
    created_at  timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT labels_scope_xor_chk
        CHECK ((org_id IS NOT NULL AND project_id IS NULL)
            OR (org_id IS NULL AND project_id IS NOT NULL)),
    CONSTRAINT labels_color_chk
        CHECK (color ~ '^#[0-9a-fA-F]{6}$')
);
CREATE UNIQUE INDEX labels_org_name_uniq
    ON workitems.labels (org_id, lower(name))
    WHERE org_id IS NOT NULL;
CREATE UNIQUE INDEX labels_project_name_uniq
    ON workitems.labels (project_id, lower(name))
    WHERE project_id IS NOT NULL;

CREATE TABLE workitems.item_labels (
    item_id    text         NOT NULL REFERENCES workitems.items(id) ON DELETE CASCADE,
    label_id   text         NOT NULL REFERENCES workitems.labels(id) ON DELETE CASCADE,
    applied_at timestamptz  NOT NULL DEFAULT now(),
    applied_by text         REFERENCES auth.users(id) ON DELETE SET NULL,
    PRIMARY KEY (item_id, label_id)
);
CREATE INDEX item_labels_label_idx ON workitems.item_labels (label_id);

-- Item ↔ milestone is 1:1 (per PRD § 6.3 "exactly one milestone"). Membership
-- is represented as the `milestone_id` column on `workitems.items` plus the
-- audit fields `milestone_assigned_at` / `milestone_assigned_by` on the
-- same row (added to the items DDL above). No junction table — the
-- earlier draft's parallel `item_milestone` table was redundant with the
-- column and the two paths had asymmetric ON DELETE policies (SET NULL vs
-- CASCADE) that risked drift. Single source of truth on the items row.
-- (`items_milestone_idx` on `workitems.items (milestone_id)` is declared
-- once with the items table indexes above; do not redeclare here.)

-- Comments (PRD § 6.5). Append-only, (kind, status) orthogonal axes.
CREATE TABLE workitems.comments (
    id         text         PRIMARY KEY,                                   -- ULID
    item_id    text         NOT NULL REFERENCES workitems.items(id) ON DELETE CASCADE,
    parent_id  text         REFERENCES workitems.comments(id) ON DELETE SET NULL,
    author_id  text         REFERENCES auth.users(id) ON DELETE SET NULL,
    author_agent text,                                                     -- AgentKind value if author is an agent
    kind       text         NOT NULL,                                      -- PRD § 6.5 kind list
    status     text         NOT NULL DEFAULT 'info',                       -- 'error' | 'warning' | 'info' | 'success'
    body       text         NOT NULL,
    created_at timestamptz  NOT NULL DEFAULT now(),
    updated_at timestamptz  NOT NULL DEFAULT now(),                            -- bumped on body edit; PRD FR-10
    CONSTRAINT comments_no_self_parent_chk
        CHECK (parent_id IS NULL OR parent_id <> id),
    CONSTRAINT comments_kind_chk
        CHECK (kind IN ('investigation', 'decision', 'deviation', 'completed',
                        'review', 'qa', 'deferred', 'pr', 'needs-human',
                        'override', 'general')),
    CONSTRAINT comments_status_chk
        CHECK (status IN ('error', 'warning', 'info', 'success')),
    CONSTRAINT comments_author_chk
        CHECK (author_id IS NOT NULL OR author_agent IS NOT NULL)
);
CREATE INDEX comments_item_created_idx ON workitems.comments (item_id, created_at);
CREATE INDEX comments_parent_idx       ON workitems.comments (parent_id);
CREATE INDEX comments_status_idx       ON workitems.comments (status);
CREATE INDEX comments_kind_status_idx  ON workitems.comments (kind, status);
-- Full-text search backing the `search` MCP tool (research AF1).
ALTER TABLE workitems.comments ADD COLUMN fts tsvector
    GENERATED ALWAYS AS (to_tsvector('english', coalesce(body, ''))) STORED;
CREATE INDEX comments_fts_idx ON workitems.comments USING GIN (fts);
