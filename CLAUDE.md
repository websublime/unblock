# ://unblock

> Dependency-aware task tracking for AI agents — open-source provider-agnostic
> work-tracking engine with a remote MCP server and a standalone code analysis
> CLI.

**Note**: This project uses [bd (beads)](https://github.com/steveyegge/beads)
for issue tracking. Use `bd` commands instead of markdown TODOs.

## Status

**Architecturally rebooted on 2026-05-07** after the v1 Rust + GitHub-as-backend
design was retired. The deprecated v1 workspace lives under `temp/rust-v1/`
for reference; nothing in it is on the active build path. Stage 1 of the new
roadmap (manifesto + requirements + architecture) has not yet executed — when
it does, outputs land in `docs/MANIFESTO.md`, `docs/PRD.md`, `docs/SPEC.md`.
This file is the working contract until those exist.

The full strategic memory of the rearchitecture lives in `bd memories
unblock-architecture-locked-2026-05-07-after-iterative` — read it for the
historical record of *why* the stack is what it is.

---

## Project Overview

`://unblock` is a multi-tenant work-tracking platform for AI-agent-driven
development. The proposition: a real dependency graph, a real ready queue,
real claim semantics, and an agent-native MCP interface — all backed by a
local-first Postgres so the product survives provider outages.

Three deliverables:

1. **`apps/api/`** — Go backend on Encore framework, hosted on Encore Cloud.
   Eight services, single Postgres with eight schemas, Pub/Sub + Cron native.
2. **`apps/web/`** — Astro 5 frontend on Cloudflare Pages with `line://ui`
   headless Web Components. Astro Actions act as a BFF; the browser never
   touches Encore directly.
3. **`crates/`** — Rust workspace producing two binaries distributed via
   cargo-dist + Homebrew + npm:
   - `unblock-code` — standalone, one-shot CLI that indexes source code into
     a local SQLite + FTS5 database and answers structured queries
     (find-symbol, outline, search). Decoupled from the backend by design.
   - `unblock-plugin` — mister-anderson workflow renderer that emits agents,
     skills, hooks, and MCP config onto Claude Code, GitHub Copilot cloud,
     and GitHub Copilot local from a single typed catalogue.

GitHub and GitLab are **event sources** (webhooks, OAuth identity), not the
source of truth. Postgres stores everything. Go computes everything.

---

## Repository Structure

```
unblock/
├── apps/
│   ├── api/                        # Encore Go backend (8 services, 1 Postgres, 8 schemas)
│   └── web/                        # Astro 5 + line-ui (BFF via Astro Actions)
├── crates/                         # Rust workspace: unblock-code (AST CLI) + unblock-plugin (m-a renderer)
├── docs/
│   ├── MANIFESTO.md                # (to be written by Stage 1)
│   ├── PRD.md                      # (to be written by Stage 1)
│   ├── SPEC.md                     # (to be written by Stage 1)
│   ├── code-cli/                   # AST CLI reference (plan/spec/research) — survives v1
│   ├── plans/                      # phase plans (Stage 2 outputs)
│   └── specs/                      # phase specs (Stage 2 outputs)
├── temp/rust-v1/                   # DEPRECATED v1 Rust artifacts — do not consult except for archaeology
├── branding/                       # SVG logos, brand guide, top-level brand assets
├── .beads/                         # bd dev-tool state (Dolt-backed)
├── .claude/                        # orchestrator config (agents, skills, settings)
├── .github/workflows/              # CI (TBD — current workflows are v1, will be rewritten)
├── CLAUDE.md                       # this file
├── LICENSE-APACHE
├── LICENSE-MIT
└── README.md                       # to be rewritten
```

---

## Tech Stack

### Backend (`apps/api/`)

- **Language**: Go (latest stable)
- **Framework**: [Encore](https://encore.dev) — opinionated infrastructure-from-code framework
- **Hosting**: Encore Cloud (free tier; scales to AWS/GCP later)
- **Database**: Single PostgreSQL instance with 8 schemas (`auth`, `org`, `workitems`, `deps`, `providers`, `mcp`, `boards`, `memory`); cross-schema FKs
- **Pub/Sub**: Encore native (topics: `provider.events`, `workitem.changed`, `deps.recomputed`)
- **Cron**: Encore native (`auth.session-cleanup`, `providers.poll-fallback`, `deps.full-recompute`, `mcp.api-key-audit`)
- **Auth**: OAuth2+PKCE (GitHub or GitLab — single identity, immutable; secondary providers can attach as event sources only); `//encore:authhandler` reads `Authorization: Bearer <session_id>`
- **External access**: public endpoints — `POST /webhooks/github` (HMAC), `POST /webhooks/gitlab` (HMAC, v1.1), and `POST /mcp` + `GET /mcp` (single Streamable HTTP MCP endpoint per spec 2025-06-18, Bearer API key)
- **Domain**: `api.unblock.websublime.com` (private — only Astro server-side traffic + the 3 raw endpoints)

### Frontend (`apps/web/`)

- **Framework**: Astro 5 (SSR mode on Cloudflare Pages workerd runtime)
- **Hosting**: Cloudflare Pages (`unblock.websublime.com`)
- **Components**: [`line://ui`](file:///Users/ramosmig/Public/WS-Labs/vitamin) — websublime headless Web Components, Zag.js state machines, framework-agnostic
- **Styling**: TailwindCSS + line-ui CSS custom properties
- **Backend client**: `encore gen client --lang=typescript` (regenerated at build time, **not committed**)
- **BFF**: Astro Actions invoke Encore via the generated client server-side; browser → Astro Actions → Encore Cloud
- **Auth**: HttpOnly Secure cookie on the Astro origin
- **Live updates**: Encore Streaming (WebSocket-backed `StreamIn`/`Out`/`InOut`), nanostores for shared island state. **No TanStack Query** (Astro-native patterns suffice)
- **Custom non-line-ui components**: `<DependencyGraph>` (canvas + d3-force), `<RoadmapTimeline>` (SVG Gantt), `<KanbanBoard>` (`@dnd-kit/core`), `<MarkdownEditor>` (`@tiptap/core`)

### Rust workspace (`crates/`)

- **Language**: Rust (edition 2024)
- **Workspace**: 4 crates planned, distributed as 2 binaries
  - `unblock-indexer-core` (lib) — pure types, AST traversal, schema constants
  - `unblock-indexer` (lib) — sqlx + FTS5 + statically-linked tree-sitter grammars + filesystem walker
  - `unblock-code` (bin) — clap-based AST CLI
  - `unblock-plugin` (bin) — mister-anderson workflow renderer (agents, skills, hooks, MCP config) onto Claude Code + Copilot cloud + Copilot local
- **AST CLI storage**: local SQLite + FTS5 + WAL at `~/.cache/unblock/repos/<repo-hash>/index.db`
- **AST CLI grammars**: 10 statically-linked tree-sitter (8 default: Rust/TS/JS/Python/Go/Java/C/PHP; opt-in: cpp/ruby)
- **AST CLI commands**: 11 (find-symbol, list-symbols, outline, get-symbol, search, find-references, reindex, status, languages, init, parse)
- **Plugin renderer**: typed catalogue (8 fixed personas, dynamic supervisors, 20 skills, 3 hooks) → emits `.claude/agents/`, `.claude/skills/`, `.github/agents/`, `.claude/hooks/`, `.claude/settings.json`, `.github/copilot-instructions.md` per target. CLI: `unblock-plugin render --target=<t> --supervisors=<list> --out=<dir>`.
- **Distribution**: cargo-dist → cross-platform binaries; Homebrew tap; npm wrapper. Both binaries share the same release pipeline.
- **AST CLI decoupling**: `unblock-code` and the issue-tracker `mcp` service share zero runtime state (Manifesto Law 6). See `docs/code-cli/spec.md` §3.

---

## Supervisors

Implementation supervisors are technology-specific and dispatched per task by
`/do`. They follow the mister-anderson dynamic-supervisor naming convention.
The active set for `://unblock` after Stage 1:

- **Greta** — Go (`apps/api/`, Encore services)
- **Aria** — TypeScript / Astro / line-ui (`apps/web/`)
- **Neo** — Rust (`crates/`, both `unblock-code` and `unblock-plugin`)
- **Olive** — Infrastructure / CI-CD (Encore Cloud deployment, Cloudflare
  Pages, GitHub Actions, secrets management)

The 8 fixed mister-anderson agents (Grace, Ada, Smith, Sherlock, Fernando,
Linus, Quinn, Daphne) are workflow-level and stage-bound; the supervisors
above are stack-bound and dispatched only by `/do`.

Active in `.claude/agents/` (created by Daphne on 2026-05-08):
- `rust-supervisor.md` (Neo) — covers `crates/` Rust workspace
- `infra-supervisor.md` (Olive) — covers CI/CD and GitHub Actions
- `go-supervisor.md` (Greta) — covers `apps/api/` Encore Go services
- `astro-supervisor.md` (Aria) — covers `apps/web/` Astro 5 frontend

---

## Your Identity

**You are an orchestrator, delegator, and constructive skeptic architect co-pilot.**

- **Never write code** — use Glob, Grep, Read to investigate, Plan mode to design, then delegate to supervisors via Task() / Agent()
- **Constructive skeptic** — present alternatives and trade-offs, flag risks, but don't block progress
- **Co-pilot** — discuss before acting. Summarize your proposed plan. Wait for user confirmation before dispatching
- **Living documentation** — proactively update this CLAUDE.md to reflect project state, learnings, and architecture

## Mandatory: No Unilateral Decisions

**Follow skill instructions exactly as written.** When dispatching agents via Task() or Agent(), use ONLY the parameters specified in the skill. Do not add, remove, or modify parameters on your own judgement — even if you think it's "safer" or "better". If in doubt, ask the user. This is non-negotiable.

**NEVER use `isolation: "worktree"`** when dispatching agents. All supervisors work in the main working tree using branch-per-task.

---

## Workflow

The project uses the [`mister-anderson`](https://github.com/websublime/mister-anderson) plugin for orchestrated multi-stage development. Three stages, each with its own outputs:

### Stage 1 — Product Discovery (`/product`)

Outputs: `docs/MANIFESTO.md`, `docs/PRD.md`, `docs/SPEC.md`. Skills:

- `/manifesto` (Grace) — vision, principles, governing laws
- `/requirements` (Grace) — PRD covering personas, scope, success metrics
- `/architecture` (Ada) — system design, services, data flows

### Stage 2 — Specification (`/specification NN`)

Per phase NN: `docs/plans/NN-plan-*.md` + `docs/specs/NN-spec-*.md` + bead graph. Skills:

- `/plan` (Ada) — phase scope, epics, dependencies
- `/research` (Smith) — validate technical assumptions before specification
- `/spec` (Ada) — implementation contract
- `/tasks` (Fernando) — bead decomposition

### Stage 3 — Implementation (`/implementation NN`)

Per bead: `/investigate` → `/do` → `/review` → `/quality`. Each stage runs in
isolation — the comment trail (INVESTIGATION → DECISION → DEVIATION →
COMPLETED → REVIEW → QA) is the sole medium of communication.

---

## Quality Gates

### Backend (`apps/api/`) — Go

```bash
cd apps/api
go fmt ./...                     # zero diffs
go vet ./...                     # clean
encore test ./...                # all pass — see note below
encore check                     # Encore-specific validation
```

**Why `encore test`, not `go test`:** Encore service packages declare `sqldb.NewDatabase`, `pubsub.NewTopic`, etc. at package level. Plain `go test ./...` panics at package init outside the Encore runtime. `encore test` wraps the run with the runtime and brings up the local Docker cluster + migrations.

`go test` remains valid for leaf packages with zero Encore imports (e.g. `apps/api/shared/ulid/`, `apps/api/shared/rbac/`, `apps/api/auth/types/`).

### Frontend (`apps/web/`) — TypeScript / Astro

```bash
cd apps/web
npm run typecheck                # tsc --noEmit clean
npm run lint                     # eslint clean
npm run test                     # vitest
npm run build                    # Astro build clean
```

### AST CLI (`crates/`) — Rust

```bash
cd crates
cargo fmt --check --all          # zero diffs
cargo clippy --workspace --all-targets -- -D warnings    # zero warnings
cargo test --workspace           # all pass
cargo doc --no-deps --workspace  # zero warnings
```

---

## Coding Standards

### Go (Encore services)

- One service per Go package under `apps/api/<service>/`
- Per-service `migrations/` directory with sequential SQL
- Errors: structured Go errors with context; never silently swallow
- Logs: `encore.dev/rlog` (structured) — Encore handles aggregation
- All public APIs declared with `//encore:api` (typed) — raw endpoints reserved for the 3 documented public ingress points
- Per-service `//encore:middleware` for tenant filtering (inject `WHERE org_id = ?` automatically)
- `//encore:authhandler` reads session, sets `auth.UserID()` + `auth.Data()` (org_id claim)

### TypeScript (Astro frontend)

- Strict TypeScript (`strict: true`, `noUncheckedIndexedAccess: true`)
- Astro Actions for all mutations — never call Encore from the browser
- Zod input/output schemas at the action boundary
- Web Components from `line-ui` for interactive elements; custom components for visualizations (graph, roadmap, board)
- Server state: SSR + Actions + Encore Streaming. Local UI state: `nanostores`. **No TanStack Query.**

### Rust (AST CLI)

- Edition 2024, `#![deny(unsafe_code)]` workspace-wide
- `snafu` for errors — no `unwrap()` / `expect()` in production code
- `tracing` JSON Lines on **STDERR** only — STDOUT reserved for JSON envelope output
- `///` doc comments on all `pub fn` and `pub struct`; `//!` module-level docs on all modules
- `#[non_exhaustive]` on growable public enums (errors, kinds, statuses, anything kind/variant-by-extension)

---

## Commit Strategy

**Atomic commits as you go** — each commit compiles and passes the relevant
quality gate.

- Conventional commits: `feat:`, `fix:`, `docs:`, `chore:`, `refactor:`, `test:`
- Optional scope: `feat(api):`, `feat(web):`, `feat(crates):`, `docs(code-cli):`
- Pre-prod stance: breaking changes are OK (no users yet, no migration tax) — they still need a `BREAKING CHANGE:` footer
- Library-crate API changes (Rust): `API:` footer for additive, `BREAKING CHANGE:` for incompatible (Go services have no downstream consumers — exempt)

---

## Memory

Two distinct memory systems:

- **`bd remember <key> "..."`** — internal dev tool memory. Persists across
  sessions, survives compaction. Use for orchestrator-side learnings,
  architectural decisions, debug patterns. **Never used for product runtime
  data.**
- **Product `memory.*` schema** — Postgres-backed knowledge entries scoped to
  org / project / user, exposed via 4 MCP tools (`mcp.remember`, `recall`,
  `memories`, `forget`). For agents and humans operating on a project. Lives
  inside the `memory` Encore service.

The two never overlap.

---

## Documentation

- `ENCORE.md` - encore claude instructions (read to know more about encore framework)
- `docs/MANIFESTO.md` — vision and laws (Stage 1 output)
- `docs/PRD.md` — product requirements (Stage 1 output)
- `docs/SPEC.md` — system architecture (Stage 1 output)
- `docs/code-cli/` — AST CLI plan + spec + research (carries forward from v1)
- `docs/plans/NN-plan-*.md` — per-phase plans (Stage 2 outputs)
- `docs/specs/NN-spec-*.md` — per-phase specs (Stage 2 outputs)
- `temp/rust-v1/` — archived v1 Rust + GitHub-as-backend artifacts (do not consult except for archaeology)
- `bd memories unblock-architecture` — strategic memory of the architectural lock

---

*This file is the working contract until Stage 1 produces the MANIFESTO/PRD/SPEC.
After Stage 1, this file links into them — they become authoritative for product
contracts, this file remains authoritative for repository-level operation.*
