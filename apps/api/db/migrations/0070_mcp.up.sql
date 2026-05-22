-- Schema `mcp` — api_keys, tool_calls.
-- Canonical DDL: docs/SPEC.md § 9.4.6.
-- FK direction: mcp -> auth, org, workitems (already migrated).
-- key_hash is bytea HMAC-SHA256 (per research C7 — NOT argon2id).

CREATE SCHEMA IF NOT EXISTS mcp;

-- Per-agent API keys. The hot-path Bearer auth check.
CREATE TABLE mcp.api_keys (
    id              text         PRIMARY KEY,                              -- ULID
    org_id          text         NOT NULL REFERENCES org.organizations(id) ON DELETE CASCADE,
    issued_to_user  text         REFERENCES auth.users(id) ON DELETE SET NULL,
    label           text         NOT NULL,                                 -- e.g. 'claude-code-laptop'
    agent_kind      text         NOT NULL,                                 -- AgentKind value
    key_hash        bytea        NOT NULL,                                 -- HMAC-SHA256(server_secret, key); 32 bytes raw.
                                                                           -- Argon2id is the wrong primitive here: keys are
                                                                           -- 32-byte URL-safe random with 256 bits of entropy,
                                                                           -- so brute force is mathematically infeasible
                                                                           -- regardless of hash speed; argon2id's per-call
                                                                           -- ~50ms cost would directly threaten NFR-1 (the
                                                                           -- p99 < 2s budget is hot-path Bearer auth).
                                                                           -- HMAC with a server-side secret prevents lookup
                                                                           -- table attacks against a leaked key_hash dump
                                                                           -- (research C7).
    key_prefix      text         NOT NULL,                                 -- first 8 chars for hint UI
    scopes          text[]       NOT NULL DEFAULT '{}',                    -- coarse scopes; tool-level RBAC is in mcp service
    created_at      timestamptz  NOT NULL DEFAULT now(),
    last_used_at    timestamptz,
    -- Optional natural expiry. NULL means "never expires by default" — at
    -- v1 we do not auto-rotate API keys. Lifecycle is operator-driven:
    -- (a) issuance happens via `auth.IssueAPIKey` (called from test seeds
    --     in P01 — the E2E test apps/api/exitcriteriontest/ writes the
    --     row straight to mcp.api_keys via direct INSERT, with key_hash
    --     computed using secrets.APIKeyHMACSecret per
    --     apps/api/auth/apikey.go:103-111; see spec §11.1.1, round-12);
    --     operator-facing surfaces (CLI / web admin) ship in a future phase.
    -- (b) Rotation is a manual two-step: issue a new key (new prefix), wait
    --     for the agent operator to switch over, then set `revoked_at` on
    --     the old row. Both rows coexist during the rollover window.
    -- (c) There is no auto-refresh, no auto-rotate, and no key-expiry
    --     scheduler in v1. `expires_at` is honoured if set (auth.Validate
    --     refuses keys past `expires_at`) but the column defaults to NULL
    --     and operators rarely set it. See research AF4.
    expires_at      timestamptz,
    revoked_at      timestamptz,
    CONSTRAINT api_keys_agent_chk
        CHECK (agent_kind IN ('claude-code', 'copilot', 'cursor', 'codex', 'aider', 'custom')),
    CONSTRAINT api_keys_prefix_uniq UNIQUE (key_prefix)
);
CREATE INDEX api_keys_org_active_idx
    ON mcp.api_keys (org_id, last_used_at DESC NULLS LAST)
    WHERE revoked_at IS NULL;

-- Tool-call audit. Every MCP call is recorded for forensics + state-machine
-- rejections analysis (Layer 1, FR-9).
CREATE TABLE mcp.tool_calls (
    id           text         PRIMARY KEY,                                 -- ULID
    api_key_id   text         REFERENCES mcp.api_keys(id) ON DELETE SET NULL,
    org_id       text         NOT NULL REFERENCES org.organizations(id) ON DELETE CASCADE,
    project_id   text         REFERENCES org.projects(id) ON DELETE SET NULL,
    item_id      text         REFERENCES workitems.items(id) ON DELETE SET NULL,
    tool_name    text         NOT NULL,
    arguments    jsonb        NOT NULL DEFAULT '{}'::jsonb,
    result_kind  text         NOT NULL,                                    -- 'ok' | 'rejected' | 'error'
    rejection_reason text,                                                 -- precondition name when result_kind='rejected'
    error_code   text,
    duration_ms  integer      NOT NULL,
    trace_id     text,
    called_at    timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT tool_calls_result_chk
        CHECK (result_kind IN ('ok', 'rejected', 'error'))
);
CREATE INDEX tool_calls_org_called_idx     ON mcp.tool_calls (org_id, called_at DESC);
CREATE INDEX tool_calls_item_idx           ON mcp.tool_calls (item_id);
CREATE INDEX tool_calls_rejected_idx
    ON mcp.tool_calls (org_id, called_at DESC)
    WHERE result_kind = 'rejected';
CREATE INDEX tool_calls_arguments_gin_idx  ON mcp.tool_calls USING gin (arguments);
