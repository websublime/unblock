# apps/api — Encore Go backend

This directory will host the Encore Go application that backs `://unblock`.

## Status

**Empty skeleton.** Bootstrap deferred until Stage 1 (manifesto + requirements + architecture)
completes. Initialization commands (when ready):

```bash
encore app create unblock --runtime=go
```

## Planned services

8 Encore services backing `://unblock`:

- `auth` — OAuth2+PKCE GitHub/GitLab, JWT, sessions, RBAC primitives
- `org` — organizations, members, projects, project visibility
- `workitems` — Epic→Issue→Task hierarchy, labels, iterations, milestones, comments
- `deps` — dependency graph engine (cycle detection, ready/blocked, transitive closure)
- `providers` — GitHub/GitLab webhook ingestion, normalization, bidirectional sync
- `mcp` — 18 MCP tools via remote SSE transport
- `boards` — Kanban + Roadmap + columns + views
- `memory` — org/project/user-scoped knowledge entries

Single Postgres database with 8 schemas (one per service), cross-schema FKs.

See the project root `CLAUDE.md` and `docs/SPEC.md` (post-Stage-1) for the architecture
contract.
