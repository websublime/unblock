-- Schema `org` — organizations, members, projects, project_members.
-- Canonical DDL: docs/SPEC.md § 9.4.2.
-- FK direction: org -> auth (already migrated in 0020).

CREATE SCHEMA IF NOT EXISTS org;

CREATE TABLE org.organizations (
    id          text         PRIMARY KEY,                                  -- ULID
    slug        text         NOT NULL,
    name        text         NOT NULL,
    description text,
    created_at  timestamptz  NOT NULL DEFAULT now(),
    updated_at  timestamptz  NOT NULL DEFAULT now(),
    deleted_at  timestamptz,
    CONSTRAINT organizations_slug_uniq UNIQUE (slug)
);

-- Roles are encoded as text + CHECK (per § 9.4.0). 4 roles at v1.0.
CREATE TABLE org.members (
    id         text         PRIMARY KEY,                                   -- ULID
    org_id     text         NOT NULL REFERENCES org.organizations(id) ON DELETE CASCADE,
    user_id    text         NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    role       text         NOT NULL,                                      -- 'owner' | 'admin' | 'member' | 'viewer'
    invited_by text         REFERENCES auth.users(id) ON DELETE SET NULL,
    created_at timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT members_role_chk
        CHECK (role IN ('owner', 'admin', 'member', 'viewer')),
    CONSTRAINT members_org_user_uniq
        UNIQUE (org_id, user_id)
);
CREATE INDEX members_user_idx ON org.members (user_id);
CREATE INDEX members_org_role_idx ON org.members (org_id, role);

CREATE TABLE org.projects (
    id          text         PRIMARY KEY,                                  -- ULID
    org_id      text         NOT NULL REFERENCES org.organizations(id) ON DELETE CASCADE,
    slug        text         NOT NULL,
    name        text         NOT NULL,
    description text,
    archived_at timestamptz,
    created_at  timestamptz  NOT NULL DEFAULT now(),
    updated_at  timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT projects_org_slug_uniq UNIQUE (org_id, slug)
);
CREATE INDEX projects_org_active_idx
    ON org.projects (org_id)
    WHERE archived_at IS NULL;

-- Project-level role override. Effective role = max(org role, project role).
CREATE TABLE org.project_members (
    id         text         PRIMARY KEY,                                   -- ULID
    project_id text         NOT NULL REFERENCES org.projects(id) ON DELETE CASCADE,
    user_id    text         NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    role       text         NOT NULL,                                      -- 'owner' | 'admin' | 'member' | 'viewer'
    created_at timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT project_members_role_chk
        CHECK (role IN ('owner', 'admin', 'member', 'viewer')),
    CONSTRAINT project_members_project_user_uniq
        UNIQUE (project_id, user_id)
);
CREATE INDEX project_members_user_idx ON org.project_members (user_id);
