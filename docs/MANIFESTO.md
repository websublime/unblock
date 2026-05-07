# ://unblock — Manifesto

**Status:** APPROVED
**Date:** 2026-05-07

---

## Vision

`://unblock` is an open-source, provider-agnostic work-tracking engine where
the dependency graph is first-class, the ready queue is computable, and AI
agents are first-class users — the workspace agents need to *manage* projects,
not just write code in them.

---

## Principles

1. **The graph is the product.** A flat list of issues is a to-do list, not
   project management. The blocking relationships between work items form a
   directed acyclic graph; everything `://unblock` does is a function of this
   graph — `ready`, `cascade`, cycle detection, topological ordering. Without
   the graph, there is no `://unblock`.

2. **Postgres stores, Go computes, agents ask.** The single Postgres database
   is the canonical store. The Go backend on Encore computes derived state
   (ready set, dependency closure, cycles) on every mutation. AI agents
   consume both via remote MCP. Provider integrations (GitHub, GitLab) are
   *event sources* — they enrich the graph; they are not the source of truth.

3. **Provider-agnostic by construction.** GitHub today, GitLab tomorrow,
   Linear or Jira eventually. The product never couples to a single
   tracker. Webhook ingestion normalises to a canonical `WorkItem`; bidirectional
   sync is opt-in per integration.

4. **Three orthogonal deliverables.** The platform ships as three independently
   useful pieces — the API+MCP backend, the Astro web client, and the
   standalone Rust AST CLI. Each is valuable on its own; together they
   compose. Coupling between them happens only at well-defined contracts.

5. **Agents are first-class users.** Every operation is exposed via MCP with
   structured input and output. The MCP server is not a proxy for a human
   tool — it is the primary interface, designed for agents that have no
   memory between sessions and need atomic operations.

6. **Sessions are sacred. The comment trail is the memory.** Investigation
   never contaminates implementation. Review never remembers writing the code.
   Each phase of the pipeline runs in an isolated session; the structured
   comment trail (`INVESTIGATION → DECISION → DEVIATION → COMPLETED → REVIEW
   → QA`) is the sole medium of communication between sessions and agents.

7. **Memory is structured, not parsed.** Beyond per-item comments, agents
   need persistent project-wide knowledge — architectural decisions, conventions,
   risks, lessons. The product exposes this as a first-class scoped memory
   service (`memory.*` schema, MCP tools), not as free-form text that anyone
   has to grep.

8. **The pipeline is the product.** Tools without process are chainsaws
   without safety guards. `://unblock` enforces a structured development
   pipeline (investigation → implementation → review → QA) where compliance
   is structurally impossible to bypass — MCP validates state transitions,
   gates have explicit preconditions, agent prompts have explicit BLOCK
   conditions.

---

## Governing Laws

These are invariants. A design that violates a law is wrong.

1. **Cascade is structural.** Closing a work item recomputes the graph and
   promotes newly unblocked dependents to `ready`. This is not optional. It
   happens via Pub/Sub on every mutation; agents do not need to know the
   dependency tree, they close their work and the system tells them what
   opened up.

2. **One graph, one truth.** The dependency graph stored in Postgres is
   authoritative. If two sources disagree (provider state, client cache,
   agent claim) the graph wins. Reconciliation is mechanical.

3. **Postgres is the source of truth.** The product never relies on provider
   APIs being live to function. Webhooks fail; provider outages do not stop
   `://unblock` from operating. Provider state is reconciled on a schedule;
   it is never the canonical store.

4. **BFF is structural.** The browser never holds backend credentials. Astro
   Actions act as the Backend-For-Frontend; HttpOnly cookies live on the
   Astro origin only; the Encore API is not directly reachable by the
   browser except for three documented public endpoints (provider webhooks,
   MCP SSE, OAuth callbacks).

5. **Claim semantics are atomic.** When an agent claims a work item, the
   transition (status change, agent identifier, timestamp) is a single
   transaction with `SELECT FOR UPDATE`. Two agents cannot claim the same
   item. This is enforced by the protocol, not by convention.

6. **Decoupled deliverables share no runtime state.** `unblock-code` (the AST
   CLI) does not consume the backend. The AST CLI's local SQLite index and
   the backend's Postgres database are independent. This is not a temporary
   simplification; it is a structural rule that keeps both components fast
   and offline-capable.

7. **The agent is one command away from productive work.** `prime → ready
   → claim` completes in under two seconds on a warm cache. If an agent
   needs more than one command to find work, the product has failed.

8. **Pipeline gates are enforced architecturally.** Three independent
   enforcement layers — MCP state-transition validation, the inspector
   running after every dispatch, agent prompt structure with explicit BLOCK
   conditions — must all be bypassed simultaneously for the pipeline to be
   violated. Structurally impossible.

---

## Out of Scope

The following are explicitly not in scope for `://unblock` at any phase:

- **Desktop application.** `://unblock` ships as web (Astro) + remote MCP
  + standalone CLI. There is no GPUI, Tauri, or Electron desktop app.
- **Code generation by the AST CLI.** `unblock-code` indexes, queries, and
  reports. It never writes code, refactors, or modifies source files.
- **Custom storage that duplicates Postgres.** No local SQLite caches inside
  the API service, no Redis-backed shadow state, no per-client serialisation.
  Postgres is enough.
- **Provider-specific UI.** When a work item maps to a GitHub issue, the
  product links to GitHub for the native experience. We do not reinvent
  GitHub's PR review or GitLab's merge request UI.
- **Replacing wikis, CMSs, or knowledge bases.** The `memory` service stores
  atomic facts, not documents. 8KB max per entry, no rich-text editor, no
  hierarchy. We are not Notion or Confluence.
- **Network-level multi-tenant isolation.** RBAC is org-level row-level
  filtering, not VPC isolation. Enterprise SOC 2-grade isolation is
  explicitly post-v1.
- **Self-hosting story for v1.** The product runs on Encore Cloud and
  Cloudflare Pages. Self-hosting Encore + Cloudflare Workers compatible
  Postgres is technically possible but not supported by us in v1.
- **Real-time collaboration on work item content.** Editing a description
  is single-user. We are not Figma or Google Docs.
- **Agent decision-making.** `://unblock` tracks state and exposes the
  graph. It does not decide what an agent should work on next, how to
  implement a task, or how to write tests. The agent decides; the platform
  informs.

---

*This manifesto is the foundation. Everything else — the PRD, the
architecture, the implementation plans — must align with these principles
and laws. A design that violates a law is wrong by definition; a feature
that requires relaxing a law is not built.*
