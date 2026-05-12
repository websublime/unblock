# apps/api — Encore Go backend

This directory will host the Encore Go application that backs `://unblock`.

## Status

**Empty skeleton.** Bootstrap deferred until Stage 1 (manifesto + requirements + architecture)
completes. Initialization commands (when ready):

```bash
encore app create unblock --runtime=go
```

## Planned services

8 domain Encore services backing `://unblock`:

- `auth` — OAuth2+PKCE GitHub/GitLab, JWT, sessions, RBAC primitives
- `org` — organizations, members, projects, project visibility
- `workitems` — Epic→Issue→Task hierarchy, labels, iterations, milestones, comments
- `deps` — dependency graph engine (cycle detection, ready/blocked, transitive closure)
- `providers` — GitHub/GitLab webhook ingestion, normalization, bidirectional sync
- `mcp` — 18 MCP tools via remote SSE transport
- `boards` — Kanban + Roadmap + columns + views
- `memory` — org/project/user-scoped knowledge entries

Plus a dedicated infrastructure service:

- `db` — schema-only migration-owner service (no API surface). Declares
  the canonical `sqldb.NewDatabase("unblock", ...)` exactly once across
  the workspace, holds the migration set for every schema, and is the
  SOLE binding authority for every consumer's database handle. Every
  domain service consumes via the canonical BindDB late-bind pattern
  (a nil `*sqldb.Database` pointer + exported `BindDB` hook in the
  service's `db.go`, populated from `apps/api/db/db.go`'s `init` — see
  SPEC §3.1) and is an equal database consumer; no domain service owns
  DDL for schemas it does not consume, and no domain service declares
  `sqldb.Named("unblock")` at package init.

Single Postgres database with 8 schemas (one per domain service), cross-schema FKs.

See the project root `CLAUDE.md` and `docs/SPEC.md` (post-Stage-1) for the architecture
contract.
