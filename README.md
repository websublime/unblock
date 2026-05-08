# ://unblock

> **Dependency-aware work tracking for AI agents.**
> Open-source · provider-agnostic · agent-native.

[![License](https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-blue)](#license)
[![Status](https://img.shields.io/badge/status-pre--1.0-orange)](#status)

---

## What is `://unblock`?

A work-tracking platform built for AI-agent-driven development. The dependency
graph is the product — agents call `ready` and get unblocked work, claim it
atomically, close it and watch dependents cascade. No flat issue lists. No
graph-traversal cost on the agent side. No provider lock-in.

Three deliverables, three orthogonal use cases:

- **Backend + remote MCP** — Encore Go services on a single Postgres with 8
  schemas. Exposes 18 MCP tools over Streamable HTTP for AI agents (Claude Code,
  GitHub Copilot, Cursor, custom). Work items, dependency graph engine,
  atomic claim, structured comment trail, scoped project memory, GitHub /
  GitLab webhook ingestion, three-layer pipeline state machine.
- **`unblock-code` AST CLI** — Rust binary that indexes source code into a
  local SQLite + FTS5 database. 11 structured queries (`find-symbol`,
  `outline`, `search`, …). Saves AI-agent tokens that would otherwise burn
  on `Glob` + `Grep` + `Read` chains. Standalone — does not consume the
  backend.
- **Astro web client** — humans-first triage and visualisation. Kanban,
  dependency graph (force-directed), recursive-milestone roadmap, comment
  threads. Backend-for-Frontend pattern via Astro Actions; the browser
  never holds Encore credentials.

The system runs **fully without the web UI** — the agent-facing surface is
canonical. The web is a humans-first layer over a system that already
operates.

---

## Status

**Pre-1.0. Architecture locked 2026-05-07.** The previous v1 design (Rust
crates + GitHub Issues as source of truth) was retired after the GitHub API
instability of 2025 / 2026 made GitHub-Issues-as-canonical-store non-viable
for AI-agent workloads. The retired artefacts live in `temp/rust-v1/`
(local-only, gitignored).

| Stage | Status | Artifact |
|---|---|---|
| Stage 1 — Manifesto | APPROVED | [`docs/MANIFESTO.md`](docs/MANIFESTO.md) |
| Stage 1 — Product Requirements | DRAFT | [`docs/PRD.md`](docs/PRD.md) |
| Stage 1 — System Architecture | pending | `docs/SPEC.md` |
| Stage 2 — Phase plans + specs | pending | `docs/plans/`, `docs/specs/` |
| Stage 3 — Implementation | pending | `apps/`, `crates/` |

Stage 1 (Product Discovery — manifesto + requirements + architecture) is
driven by the [mister-anderson](https://github.com/websublime/mister-anderson)
plugin.

---

## Roadmap

| Phase | Deliverable | Ships at |
|---|---|---|
| **P01** | Backend MVP — Encore Go + Postgres + 14 MCP tools | v1.0 |
| **P02** | Backend complete — providers (GitHub) + 18 MCP tools + Layer 1 enforcement | v1.0 |
| **P03** | AST CLI (`unblock-code`) v1.0.0 | v1.0 |
| **P04** | mister-anderson plugin renderer — Layer 2 + 3 enforcement | v1.0 |
| **P05** | Astro web (kanban + dep graph + roadmap + comments) | v1.1 (line-ui blocked) |

**v1.0 launches headless** — the agent-facing surface (backend MCP + AST CLI
+ plugin pipeline) is fully usable without a web UI. **v1.1** adds the web
client once [`line-ui`](https://github.com/websublime/vitamin) (websublime's
headless Web Components library) reaches feature-complete v1.

---

## Architecture (high-level)

```
┌─ apps/api/ ──────────────────────────────────────┐
│  Encore Go — 8 services, 1 Postgres / 8 schemas │
│                                                  │
│  auth · org · workitems · deps · providers       │
│  mcp · boards · memory                           │
│                                                  │
│  3 raw public endpoints:                         │
│    POST /webhooks/github                         │
│    POST /webhooks/gitlab  (v1.1)                 │
│    GET  /mcp/sse                                 │
└──────────────────────────────────────────────────┘
           ▲                          ▲
           │ Astro Actions BFF        │ MCP / SSE
           │ (server-side only)       │ (Bearer API key)
┌──────────┴──────┐    ┌──────────────┴────────────┐
│ apps/web/        │    │ AI agents                 │
│ Astro 5 + line-ui│    │ (Claude Code, Copilot, …) │
│ Cloudflare Pages │    └───────────────────────────┘
└──────────────────┘

         ┄┄ standalone, no shared state ┄┄
┌────────────────────────────────────────┐
│ crates/                                │
│   unblock-code (AST CLI, Rust)         │
│   unblock-plugin (m-a renderer, Rust)  │
│   Local SQLite + FTS5 (CLI only)       │
└────────────────────────────────────────┘
```

---

## Stack

- **Backend** — Go on the [Encore](https://encore.dev) framework, deployed
  on Encore Cloud (`api.unblock.websublime.com`)
- **Database** — Single PostgreSQL with 8 schemas (cross-schema FKs); ULID
  primary keys; `pgcrypto` symmetric encryption for sensitive columns
- **Async / Cron** — Encore native Pub/Sub (`provider.events`,
  `workitem.changed`, `deps.recomputed`) and Cron primitives
- **Frontend** — [Astro 5](https://astro.build) on Cloudflare Pages
  (`unblock.websublime.com`) +
  [`line-ui`](https://github.com/websublime/vitamin) (websublime headless
  Web Components on top of Zag.js state machines)
- **Frontend ↔ Backend** — `encore gen client --lang=typescript` (regenerated
  at build time) consumed by Astro Actions; the browser never reaches Encore
  directly
- **AST CLI** — Rust (edition 2024) workspace; `tree-sitter` + 10 statically
  linked grammars (8 default: Rust, TypeScript, JavaScript, Python, Go,
  Java, C, PHP; opt-in: C++, Ruby); `sqlx` (sqlite + FTS5 + WAL); `clap`
- **MCP transport** — Remote MCP over Streamable HTTP per spec 2025-06-18 (`POST /mcp` + `GET /mcp`); agents authenticate with
  `Authorization: Bearer <api_key>`
- **Auth** — OAuth2 + PKCE via GitHub or GitLab; HttpOnly Secure cookie on
  the Astro origin
- **Distribution** — `cargo-dist` cross-platform binaries +
  [Homebrew](https://brew.sh) tap + npm wrapper for both Rust binaries

---

## Repository structure

```
unblock/
├── apps/
│   ├── api/                Encore Go backend (8 services)
│   └── web/                Astro 5 + line-ui (P05, v1.1)
├── crates/                 Rust workspace
│   ├── unblock-indexer-core/   pure types, AST traversal
│   ├── unblock-indexer/        sqlx + FTS5 + grammars
│   ├── unblock-code/           AST CLI binary
│   └── unblock-plugin/         mister-anderson renderer binary
├── docs/
│   ├── MANIFESTO.md            vision · principles · governing laws (immutable)
│   ├── PRD.md                  product requirements (personas, scope, phasing)
│   ├── SPEC.md                 system architecture (post-Stage-1)
│   ├── code-cli/               AST CLI design (plan, spec, research)
│   ├── plans/                  per-phase plans (Stage 2 outputs)
│   └── specs/                  per-phase specs (Stage 2 outputs)
├── branding/                   SVG logos, brand guide
├── temp/rust-v1/               retired v1 artefacts (gitignored, archaeology only)
├── CLAUDE.md                   operator manual for the Claude orchestrator
├── LICENSE-APACHE
├── LICENSE-MIT
└── README.md                   this file
```

---

## Documentation

| Document | Purpose |
|---|---|
| [`docs/MANIFESTO.md`](docs/MANIFESTO.md) | Vision, 8 principles, 8 governing laws, out-of-scope. **Immutable.** |
| [`docs/PRD.md`](docs/PRD.md) | Product requirements — personas, functional / non-functional requirements, product catalogues (state model, milestones, comments, findings, personas, skills), phasing, metrics, risks |
| [`docs/SPEC.md`](docs/SPEC.md) | System architecture — Postgres DDL, MCP tool signatures, service interfaces, Pub/Sub topic schemas (post-Stage-1) |
| [`docs/code-cli/`](docs/code-cli/) | AST CLI plan, spec, research (carries forward verbatim from the v1 Phase 03 design) |
| [`CLAUDE.md`](CLAUDE.md) | Repository operator manual — orchestrator role, supervisor map, quality gates per language, coding standards |

---

## Operating model

`://unblock` is built using the
[mister-anderson](https://github.com/websublime/mister-anderson) plugin —
an agent-orchestrated development workflow. Three stages, each with its own
structured artifacts and quality gates:

1. **Stage 1 — Product Discovery** → `docs/MANIFESTO.md`, `docs/PRD.md`,
   `docs/SPEC.md`
2. **Stage 2 — Phase Specification** → `docs/plans/NN-plan-*.md` +
   `docs/specs/NN-spec-*.md` per phase
3. **Stage 3 — Implementation** → per-bead pipeline:
   `/investigate → /do → /review → /quality`

Eight workflow personas (Grace, Ada, Smith, Sherlock, Fernando, Linus,
Quinn, Daphne) plus stack-specific dynamic supervisors (Greta = Go, Aria
= Astro / TypeScript, Neo = Rust, Olive = Infra / CI-CD) drive the
pipeline. See [`CLAUDE.md`](CLAUDE.md) for the full operator manual.

---

## Contributing

Pre-1.0; **not currently accepting external contributions**. Architectural
decisions are still being locked. Open issues for ideas; pull requests are
welcome but may be deferred until v1.0 ships.

---

## License

Dual-licensed under either [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE) at your option.
