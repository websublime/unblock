# PRD: ://unblock — Provider-Agnostic Work-Tracking Engine for AI Agents

**Status:** APPROVED
**Author:** Grace (product-manager)
**Date:** 2026-05-07
**Companion:** [docs/MANIFESTO.md](./MANIFESTO.md) (APPROVED, 2026-05-07)
**Source design (carries forward verbatim):** [docs/code-cli/plan.md](./code-cli/plan.md), [docs/code-cli/spec.md](./code-cli/spec.md), [docs/code-cli/research.md](./code-cli/research.md)

---

## 1. Problem Statement

AI agents are increasingly the primary contributors to software work — investigating, implementing, reviewing, shipping. The work-tracking layer they depend on, however, was designed for humans: flat issue lists, free-form bodies, GUI-first workflows, and tools that assume continuous memory between sessions.

The concrete failure modes observed today:

1. **Flat lists, not graphs.** Existing trackers (GitHub Issues, Linear, Jira) treat dependencies as link metadata, not as a first-class structure that can be queried for "what is ready to work on right now". Agents must read every issue, parse free-form text, infer relationships, and decide manually — burning context on work the platform should do.
2. **Provider lock-in.** A team that adopts a tracker is shackled to that vendor's data model. There is no neutral, computable graph that survives a provider migration.
3. **No structured project memory.** Architectural decisions, conventions, risks, and lessons are buried in PR descriptions, Slack threads, and wiki pages. Agents have no scoped, queryable substrate for "what does this org/project expect of me?".
4. **No structural pipeline discipline.** Tools without process are chainsaws without safety guards. Today an agent can mark work `done` without investigation, without review, without QA — because the platform never enforced the pipeline. Discipline is documentation, not architecture.
5. **Code-search tax.** Agents burn enormous token budgets re-discovering codebase structure on every session via `Glob` + `Grep` + `Read` chains. There is no fast, structured query surface for symbols, definitions, and outlines.

The market does not currently offer a single product that solves all five. `://unblock` is that product: a provider-agnostic work-tracking engine where the dependency graph is the product, the ready queue is computable, project memory is structured, the pipeline is structurally enforced, and an independent AST CLI saves tokens for code navigation.

---

## 2. Objectives

- **OBJ-1.** Ship a single product surface — backend + web + AST CLI — at v1.0 such that an AI agent can go from cold start to productive work in one MCP call sequence (`prime → ready → claim`) under two seconds on a warm cache.
- **OBJ-2.** Make the dependency graph the canonical computational primitive: every mutation recomputes derived state (ready set, dependency closure, cycles), and cascades propagate via Pub/Sub without agent participation.
- **OBJ-3.** Decouple the product from any single tracker by treating Postgres as the source of truth and provider integrations (GitHub at v1.0, GitLab at v1.1) as event sources only.
- **OBJ-4.** Enforce the four-stage pipeline (investigation → implementation → review → QA) through three independent architectural layers — MCP state-transition validation, a post-dispatch state validator running after every dispatched session, and agent prompt structure with explicit BLOCK conditions — such that all three must be bypassed simultaneously to violate the pipeline.
- **OBJ-5.** Provide a first-class scoped memory service (`memory.*`) so org-/project-/user-level knowledge is structured, queryable, and sanitised — not free-form text.
- **OBJ-6.** Ship a structurally decoupled AST CLI (`unblock-code`) that demonstrably saves tokens for AI agents on representative code-navigation flows, distributed via cargo-dist, Homebrew, and npm.

---

## 3. Target Users

### 3.1 Primary persona — AI-agent-driven dev teams

Engineering organisations whose day-to-day execution layer is one or more AI agents (Claude Code, GitHub Copilot CLI, Cursor agents, custom Anthropic/OpenAI agent harnesses). Their pain: agents have no shared memory, no structured pipeline, and no fast view of "what's ready". They need a backend that exposes the graph as a tool, not as a UI.

### 3.2 Secondary persona — Orchestrators

Human or agent operators who dispatch sub-agents across a graph of work. They need a deterministic way to pick the next ready item, to enforce that an investigation agent actually wrote an `INVESTIGATION` comment before an implementation agent claims the same item, and to cascade newly unblocked work without manual bookkeeping.

### 3.3 Tertiary persona — Developers

Human engineers who interact with the system through the web UI for ceremony tasks (kanban triage, comment review, dependency visualisation) and through the CLI for code navigation. They are not the primary target; the product is designed for their agents first, for them second.

---

## 4. User Stories (JTBD-framed)

### 4.1 Primary persona — AI agent

- **US-1 — Find ready work.**
  When I (an agent) start a fresh session with no memory of prior context, I want to call a single MCP tool and receive the next ready work item with all blocking dependencies resolved, so I can begin productive work without consuming tokens on graph traversal.
  - Acceptance: a `ready` MCP call returns at least one work item whose dependency closure is fully `done`, ordered deterministically, in p99 under 2 seconds on a warm cache.
  - Acceptance: the response carries enough context (project, parent epic, comment trail, scoped memory) for the agent to start without follow-up reads.

- **US-2 — Claim atomically.**
  When two agents observe the same ready item, I want the platform to grant the claim to exactly one of them, so I never duplicate work or waste compute.
  - Acceptance: `claim` is a single Postgres transaction with `SELECT FOR UPDATE`; the loser receives a structured "already claimed" error referencing the winner's agent identifier and timestamp.

- **US-3 — Cascade automatically.**
  When I close a work item, I want the platform to recompute the graph and promote newly unblocked dependents to `ready` without me having to know what depends on what, so my mental model is "close my work, the system tells me what opened up".
  - Acceptance: a successful close emits a Pub/Sub event whose subscriber promotes newly unblocked items in the same logical operation; the next `ready` call reflects the new set.

- **US-4 — Read structured project memory.**
  When I need to know "what conventions does this project enforce?" or "what was the decision on X?", I want to query a scoped memory service and receive atomic facts, so I do not parse Slack/PRs/wikis.
  - Acceptance: `recall` returns memories scoped to org/project/user with full-text and tag filtering; values are capped at 8 KB; secrets are sanitised with a warning before storage.

- **US-5 — Be blocked when out of pipeline.**
  When I try to mark a work item `done` without an `INVESTIGATION` and `REVIEW` comment in the trail, I want the MCP server to refuse the transition, so the pipeline cannot be silently skipped.
  - Acceptance: state-transition validation rejects the mutation with a structured error citing the missing precondition.
  - Acceptance: the post-dispatch validator running after my session would also catch the violation; the agent prompt I run under contains an explicit BLOCK condition for the same case. All three layers agree.

### 4.2 Secondary persona — Orchestrator

- **US-6 — Dispatch by readiness, not by guess.**
  When I am orchestrating a fleet of agents, I want a single API that returns the n highest-priority ready items, so I can dispatch in parallel without manually traversing the graph.
  - Acceptance: `ready --limit n` returns up to n items with stable ordering and per-item metadata sufficient to dispatch.

- **US-7 — See cycle violations early.**
  When I introduce a new dependency, I want the platform to reject the operation if it would create a cycle, so the graph is always a DAG.
  - Acceptance: `add_dependency` rejects on cycle creation with a structured error pointing at the offending edge; the rejection is enforced at write time, not at read time.

### 4.3 Tertiary persona — Developer

- **US-8 — Visualise the graph in a browser.**
  When I want to understand the shape of work, I want to open a web UI showing kanban + dependency graph + roadmap, so I can communicate with stakeholders without exporting screenshots.
  - Acceptance: the Astro web client renders kanban, dependency graph, roadmap, and per-item comments; auth is via OAuth2+PKCE to GitHub or GitLab; backend credentials never reach the browser (BFF-only, HttpOnly cookie on Astro origin).

- **US-9 — Save tokens on code search.**
  When I (or an agent) need to find a symbol's definition, I want to invoke a single CLI command that returns a JSON envelope with the file, span, and signature, so I do not chain `Glob` + `Grep` + `Read`.
  - Acceptance: `unblock-code find-symbol <name>` returns p99 < 10 ms on the medium representative corpus, warm cache; envelope schema is locked per the spec.
  - Acceptance: ROI harness shows the indexer median is at least 2.0× faster than the `Glob/Grep/Read` baseline across 3 representative agent flows × N=10 runs (SOFT gate; release-publish, follow-up bead if missed, does not block release).

- **US-10 — Install the CLI without dev tooling drama.**
  When I want to install `unblock-code`, I want to run a single one-liner from cargo-dist, Homebrew, or npm, so I do not have to compile from source.
  - Acceptance: cargo-dist publishes prebuilt artifacts for Linux x86_64, macOS aarch64, and Windows x86_64; Homebrew formula and npm wrapper redistribute the same artifacts.

---

## 5. Functional Requirements

### 5.1 Backend (Encore Go API + MCP) — P01 + P02

- **FR-1.** Single Postgres database with 8 schemas: `auth`, `org`, `workitems`, `deps`, `providers`, `mcp`, `boards`, `memory`. No additional persistent stores.
- **FR-2.** OAuth2+PKCE identity via GitHub or GitLab, single primary identity per user; secondary providers attach as event sources only.
- **FR-3.** Org-level RBAC enforced as Postgres row-level filtering, applied uniformly to every read and write path. Cross-tenant leaks are a release-blocker.
- **FR-4.** Work-item domain: id, title, body, status, priority, agent claim, parent (epic), arbitrary tags, provider links. CRUD via MCP and via Astro Actions BFF.
- **FR-5.** Dependency graph stored as edges in `deps`; computed views for ready set, dependency closure, cycle detection. Recomputation on every mutation.
- **FR-6.** Cascade on close: a successful close emits a Pub/Sub event whose subscriber promotes newly unblocked dependents to `ready` in the same logical operation.
- **FR-7.** Atomic claim: `SELECT FOR UPDATE` transaction; exactly one agent wins; loser receives structured rejection.
- **FR-8.** MCP server over **Streamable HTTP** per the MCP 2025-06-18 spec — single endpoint at `/mcp` accepting both `POST /mcp` (client requests; response can be a single JSON-RPC reply or an SSE stream of incremental responses for long-running tools) and `GET /mcp` (server-initiated SSE stream for resumable / long-lived sessions). Bearer `<api-key>` auth on every request, `Mcp-Session-Id` response header on `initialize`. Implementation uses `github.com/modelcontextprotocol/go-sdk` (the canonical Go SDK; not `rmcp` which is Rust-only). Exposing 18 tools at v1.0:
  - 14 in P01 (work-item CRUD, dependencies, ready, claim, close, comment trail, prime, etc.)
  - +4 memory tools in P02 (`remember`, `recall`, `memories`, `forget`)
  - Plus the providers/sync tooling needed for bidirectional GitHub sync.
  Exact tool inventory is pinned by the architect in `docs/SPEC.md`.
- **FR-9.** State-transition validation at the MCP layer: every status change is checked against the pipeline state machine; invalid transitions are rejected with a structured error citing the missing precondition (Law 8, layer 1).
- **FR-10.** Comment trail with **two orthogonal axes**, designed so agents can act on signal without parsing free-form text (Manifesto Principle 6):
  - **`kind`** — semantic category. Eleven values: `investigation`, `decision`, `deviation`, `completed`, `review`, `qa`, `deferred`, `pr`, `needs-human`, `override`, `general`.
  - **`status`** — action signal. Four values: `error`, `warning`, `info`, `success`. **NOT NULL, default `info`.**
  - **Composition is policy-free.** A `kind=qa` with `status=success` represents PASS; with `status=error` represents FAIL. A `kind=review` with `status=success` represents APPROVE; with `status=warning` represents NEEDS-REWORK. The product does not impose a cross-axis validation matrix — agents and humans can use any (kind, status) combination that fits their use.
  - **UI uses `status`** for colour-coding and badges (line-ui alert variants map 1:1 to the four status values). **Queries filter on `status`** for cross-item inspection panels (e.g., "show every `status=error` comment in this project").
  - Comments are append-only (no in-place edits to the body after creation). Edits use `edited_at` and a versioned audit trail at the schema level (post-v1) — for v1, we only record `updated_at`.
- **FR-11.** GitHub provider integration:
  - Webhook ingestion at the public `POST /webhooks/github` endpoint, signature-verified, normalising to canonical `WorkItem`.
  - Bidirectional sync, opt-in per integration: changes in `://unblock` propagate to GitHub Issues; webhook events update the canonical store.
  - Reconciliation on a schedule when webhooks are missed or the provider is offline.
- **FR-12.** Public Encore endpoints at v1.0 (the only paths reachable directly from external services):
  - `POST /webhooks/github` — provider event sink, HMAC-verified.
  - `POST /mcp` + `GET /mcp` — remote MCP for AI agents over Streamable HTTP per the 2025-06-18 spec. Single logical endpoint at one path, two HTTP methods. Bearer `<api-key>` on every request.
  At v1.1, a third endpoint is added:
  - `POST /webhooks/gitlab` — provider event sink, HMAC-verified (v1.1).
  All other Encore APIs are private; the Astro BFF is the sole privileged client. **The OAuth callback is hosted by Astro on the web origin** (`unblock.websublime.com/auth/[provider]/callback`) — the Astro Action handles the callback and calls Encore's private `auth.exchangeOAuthCode` internally. This preserves the BFF discipline (Law 4): no auth credentials cross domain boundaries.
- **FR-13.** Memory service:
  - 4 MCP tools: `remember`, `recall`, `memories`, `forget`.
  - 3 scopes: `org`, `project`, `user`.
  - 8 KB max value size per entry.
  - Always-on secret sanitiser: detects credential-shaped strings, emits a warning, stores the sanitised form.
  - Query surface: GIN `tsvector` full-text index plus tag index. No versioning at v1.

### 5.2 Pipeline enforcement (Law 8) — P02 + P04

- **FR-14.** Layer 1 — MCP state-transition validation. State machine encoded in the backend; every transition is gated by explicit preconditions (e.g. `done` requires a `REVIEW` comment present; `claim` requires status `ready`). Implemented in P02.
- **FR-15.** Layer 2 — Post-dispatch state validator. The `verify-state` plugin hook (Stop / agentStop event) calls MCP `verify_can_transition` against the dispatched session's final state and the comment trail. Non-compliance is surfaced as a finding work item (`type=finding`, severity per §6.10) linked to the parent epic via `parent_id` and `discovered_from_id`. **No separate inspector agent is required** — the validator runs as a hook calling the same MCP machinery that enforces Layer 1.
- **FR-16.** Layer 3 — Agent prompt structure. The mister-anderson plugin renderer emits prompts onto Claude Code and GitHub Copilot with explicit BLOCK conditions matching the MCP state machine. All three layers agree by construction; bypassing any single layer leaves the other two enforcing.

### 5.3 Astro web frontend — P05

- **FR-17.** Astro 5 + line-ui (websublime headless Web Components on top of Zag.js), deployed to Cloudflare Pages.
- **FR-18.** Astro Actions act as the BFF: HttpOnly cookie on the Astro origin only; the browser never holds an Encore credential.
- **FR-19.** Views at v1.0: kanban board, dependency graph, roadmap, per-item comment trail.
- **FR-20.** Auth flow: OAuth2+PKCE to GitHub or GitLab, callback handled by the Astro origin, session cookie set on the Astro origin, Encore API reached only via Astro Actions.
- **FR-21.** Mobile-responsive layout via line-ui defaults. Native mobile clients are out of scope (see §9).

### 5.4 AST CLI (`unblock-code`) — P03

The AST CLI carries forward verbatim from the approved `docs/code-cli/{plan,spec,research}.md`. The functional requirements below summarise the surface; the spec is authoritative.

- **FR-22.** Three new Rust crates: `unblock-indexer-core` (pure lib), `unblock-indexer` (impure lib), `unblock-code` (clap-based bin).
- **FR-23.** Statically-linked tree-sitter grammars for 10 languages: Rust, TypeScript, JavaScript, Python, Go, Java, C, C++, Ruby, PHP. **8 default-enabled** (Rust, TypeScript, JavaScript, Python, Go, Java, C, PHP); **`lang-cpp` and `lang-ruby` are opt-in** Cargo features.
- **FR-24.** SQLite + FTS5 + WAL local index, cache root `~/.cache/unblock/repos/<repo-hash>/index.db`. Span-only — no body text in DB.
- **FR-25.** 11 CLI commands: `find-symbol`, `list-symbols`, `outline`, `get-symbol`, `search`, `find-references` (HEURISTIC), `reindex`, `status`, `languages`, `init`, `parse`.
- **FR-26.** 17 canonical `SymbolKind` variants persisted in SQLite with FTS5 over `name`, `signature`, `comment`.
- **FR-27.** Per-query mtime check is the **sole** sync mechanism between one-shot CLI invocations (invariant). No daemon, no watcher.
- **FR-28.** Distribution: cargo-dist prebuilt artifacts for Linux x86_64, macOS aarch64, Windows x86_64; Homebrew formula; npm wrapper.
- **FR-29.** Structurally decoupled from the backend (Manifesto Law 6): no shared runtime state between `unblock-mcp` and `unblock-code`, no cross-binary queries, no shared HTTP client.

### 5.5 Priority Classification

| Requirement | Impact | Confidence | Effort | Category |
|-------------|--------|------------|--------|----------|
| FR-1 (single Postgres, 8 schemas) | H | H | M | Must-have |
| FR-5 (dependency graph + cycle detection) | H | H | H | Must-have |
| FR-6 (cascade on close) | H | H | M | Must-have |
| FR-7 (atomic claim) | H | H | L | Must-have |
| FR-8 (MCP Streamable HTTP, 18 tools) | H | H | H | Must-have |
| FR-9 (state-transition validation, layer 1) | H | H | M | Must-have |
| FR-11 (GitHub webhooks + bidirectional sync) | H | M | H | Must-have |
| FR-13 (memory service) | H | M | M | Performance |
| FR-14–16 (pipeline three layers) | H | M | H | Must-have |
| FR-17–21 (Astro web) | M | H | H | Performance |
| FR-22–29 (AST CLI) | H | M | H | Performance |
| US-9 ROI ≥ 2.0× | M | M | M | Delight (SOFT) |
| Mobile responsive (FR-21) | L | H | L | Delight |

---

## 6. Product Catalogues

These catalogues pin product-level decisions that shape every downstream artefact. They are not implementation contracts (those live in `docs/SPEC.md`) — they define the domain shapes, workflow surface, and behavioural rules that the architect, supervisors, and agents all reason against.

A reminder of what is **deliberately absent** from these catalogues, having been v1 GitHub-API workarounds: pipeline-state labels, `derive_label` mappings, label reconciliation invariants, custom-field "smushed" canonical names, comment kinds parsed from text prefixes. Postgres is the source of truth; the UI computes display from typed columns; agents act on enums, not on parsed strings.

### 6.1 Custom field enums

Persisted as Postgres enum columns on `workitems.items` (no derive layer):

| Enum | Values | Notes |
|---|---|---|
| `Status` | `Backlog`, `Ready`, `InProgress`, `Blocked`, `Done` | Computed; `is_ready` materialised by `deps` after every mutation; UI / MCP read the column |
| `Priority` | `P0`, `P1`, `P2`, `P3`, `P4` | Wire value is the code only. UI maps to display labels (e.g. `P0` → "Critical") at render time, not at storage time |
| `PipelineStage` | `Investigation`, `Implementation`, `Review`, `Quality`, `Deferred`, `Done` | Pipeline progress dimension |
| `AgentKind` | `claude-code`, `copilot`, `cursor`, `codex`, `aider`, `custom` | Identifier for the agent that claimed the item; attached to `claimed_by_agent` |

### 6.2 State model — three orthogonal dimensions

Pipeline state lives as three enum columns on `workitems.items`. **No derived label, no label reconciliation.** The Astro web client computes any display badge it wants by reading the three columns directly via Encore RPC.

| Dimension | Column | Values |
|---|---|---|
| Implementation | `impl_state` | `pending`, `done` |
| Review | `review_state` | `pending`, `approved`, `needs_rework` |
| QA | `qa_state` | `pending`, `passed`, `failed` |

**Invariants** (enforced at MCP layer, FR-9):

- Writing `review_state=needs_rework` resets `qa_state=pending` in the same transaction.
- Writing `qa_state=failed` requires `review_state=approved` (otherwise rejected).
- After `qa_state=failed`, the next supervisor `claim` resets `review_state=pending` + `qa_state=pending` atomically.
- `impl_state=done` is required before `review_state` can leave `pending`.
- Transitioning `impl_state=done → pending` is allowed only via the rework path (Review NEEDS-REWORK or QA FAIL routes).

**Exception modes** are first-class enum values on a separate column `pipeline_state` (overrides the three dimensions for orchestration purposes):

| `pipeline_state` value | Meaning |
|---|---|
| `running` | Default — the three dimensions above are authoritative |
| `needs_human` | Escape valve: 3× rework (review or qa), claim conflict, worktree conflict, or manual flag |
| `paused` | User-paused; resume restores the prior state |
| `no_investigation` | Set by `/plan` or developer to skip the investigation step in the pipeline |

### 6.3 Milestone hierarchy (recursive)

Milestones are **recursive containers** that group work items in time. A milestone can contain child milestones, supporting the typical Quarter → Sprint pattern as well as deeper structures (Year → Quarter → Month → Sprint).

The v1 schema's `workitems.iterations` table is **dropped** — recursive milestones absorb the iteration concept. A sprint is simply a leaf milestone with a short date range; an iteration is just a milestone you happen to use as a sprint.

#### Structure example

```
Quarter Q1 (milestone, 90 days)
├── Sprint 1.1 (milestone, 15 days)
├── Sprint 1.2 (milestone, 15 days)
├── Sprint 1.3 (milestone, 15 days)
├── Sprint 1.4 (milestone, 15 days)
├── Sprint 1.5 (milestone, 15 days)
└── Sprint 1.6 (milestone, 15 days)
```

#### Schema shape

A milestone has: `id` (ULID), `parent_milestone_id` (nullable self-reference FK), `org_id` XOR `project_id` (same scoping as labels), `name`, `description`, `start_date`, `end_date`, `cancelled_at` (nullable), and a derived `status` view (see below).

#### Membership (1:1)

A work item can belong to **exactly one** milestone (modelled as a `milestone_id` column on `workitems.items`, with `milestone_assigned_at` / `milestone_assigned_by` audit columns on the same row — single source of truth, no junction table). The milestone can be at **any level** of the tree:

- An epic that spans the whole quarter → `milestone_id = Q1`
- A task assigned to a specific sprint → `milestone_id = Sprint 1.3`

Quarter-level membership of an item assigned to Sprint 1.3 is **derived** by walking the parent chain (recursive CTE) — items "in Q1" includes both directly-assigned epics and tasks assigned to any descendant sprint.

#### Invariants

| # | Invariant | Enforcement |
|---|---|---|
| **M-INV-1** | `id != parent_milestone_id` — no self-loop | DB CHECK |
| **M-INV-2** | Cycle prevention across the parent chain | App-level (recursive CTE check on insert / update) |
| **M-INV-3** | Child date range ⊆ parent date range | App-level invariant |
| **M-INV-4** | Sibling time overlap is **allowed with warning** — some teams overlap sprints during transitions | App-level warning, not hard reject |
| **M-INV-5** | Child's `(org_id, project_id)` scope matches parent's | App-level + DB constraint |
| **M-INV-6** | Max depth = 4 (year / quarter / month / sprint covers all known patterns) | App-level enforcement |
| **M-INV-7** | Item's milestone scope must be reachable in the item's project (item in project P can only be in milestones scoped to P or to P's org) | App-level enforcement |

#### Derived `status`

Not a stored column — a computed view from `start_date`, `end_date`, `cancelled_at`, and the descendant items' `Status`:

| Value | Condition |
|---|---|
| `upcoming` | `start_date > now()` AND `cancelled_at IS NULL` |
| `active` | `now()` ∈ `[start_date, end_date]` AND `cancelled_at IS NULL` |
| `completed` | `end_date < now()` AND every descendant item has `Status = Done` AND `cancelled_at IS NULL` |
| `overdue` | `end_date < now()` AND any descendant item has `Status != Done` AND `cancelled_at IS NULL` |
| `cancelled` | `cancelled_at IS NOT NULL` |

The Astro web client renders milestone progress directly from this view; agents query it via MCP.

#### Query patterns unlocked

| Need | Query shape |
|---|---|
| Items directly in Q1 (epics not assigned to any sprint inside Q1) | `SELECT * FROM workitems.items WHERE milestone_id = q1_id` |
| All items inside Q1 (epics + sprint-assigned tasks) | Recursive CTE walking down from Q1; aggregate items pointed to by Q1 or any descendant |
| Burndown per sprint | `SELECT closed_at, count(*) FROM workitems.items WHERE milestone_id = sprint_id GROUP BY date_trunc('day', closed_at)` |
| Roadmap render | Tree of milestones (recursive CTE) + per-node item counts and `status` view aggregated upward |

#### MCP surface

The `mcp` service exposes milestone CRUD via the standard work-item tools. Specific shape lands in `docs/SPEC.md`. At minimum:

- `milestones.create({ parent_milestone_id?, name, description, start_date, end_date, scope })`
- `milestones.assignItem({ item_id, milestone_id })` / `milestones.unassignItem({ item_id })`
- `milestones.tree({ root_id?, scope })` — return the tree (recursive CTE)
- `milestones.cancel({ id, reason })` — sets `cancelled_at`

### 6.4 User-facing labels (not pipeline state)

`workitems.labels` is a real Postgres table — these are user-facing classification labels (bug, feature, tech-debt, breaking-change, customer-impact) that humans and agents apply to work items for filtering and querying. They are **not** state-derived.

- Scope: `org_id` XOR `project_id` (CHECK constraint enforces XOR — labels are either org-wide or project-local).
- Each label has `name`, `color`, `description`.
- Many-to-many junction table `workitems.item_labels`.
- Labels can be filtered in `ready`, `list`, and board views.
- Project labels override identically-named org labels (resolved at query time, project wins).

Pipeline state, severity findings, and exception modes are **not** expressed via labels — see §6.2 (state and exception modes), §6.6 (findings), §6.11 (rework paths).

### 6.5 Comment trail — kind × status orthogonal axes

Per FR-10. Comments are append-only structured records with two axes.

| `kind` (semantic category) | Canonical position in the pipeline |
|---|---|
| `investigation` | After `claim`, before implementation |
| `decision` | Any point during implementation; design choices |
| `deviation` | Any point; documented divergence from plan / spec |
| `completed` | At end of implementation; `impl_state` → `done` |
| `review` | After `completed`; `review_state` → `approved` or `needs_rework` |
| `qa` | After review approved; `qa_state` → `passed` or `failed` |
| `deferred` | Any point; explains a deferred sub-task or bead |
| `pr` | When a pull request is opened or merged that closes the item |
| `needs-human` | When `pipeline_state` → `needs_human` |
| `override` | When QA FAIL is bypassed via the override path |
| `general` | Catch-all; ad-hoc human or agent prose |

**`status` axis** (orthogonal action signal): `error`, `warning`, `info`, `success`. NOT NULL, default `info`. UI uses the four values for colour-coded badges (mapping 1:1 to line-ui alert variants); queries filter `status` for "show all errors / warnings on this item / project".

The product **does not impose** a (kind, status) cross-axis validation matrix — agents and humans use whatever combination expresses the truth. Common pairings:

| Pair | Meaning |
|---|---|
| `kind=qa, status=success` | QA PASS |
| `kind=qa, status=error` | QA FAIL |
| `kind=qa, status=warning` | QA PASS with findings (recorded but non-blocking) |
| `kind=review, status=success` | Review APPROVE |
| `kind=review, status=warning` | Review NEEDS-REWORK |
| `kind=completed, status=success` | Implementation finished cleanly |
| `kind=needs-human, status=error` | Escape valve fired |
| `kind=override, status=warning` | QA bypassed; risk acknowledged |

### 6.6 Findings as first-class child work items

V1 expressed review / QA findings as label suffixes on the originating bead. The new architecture surfaces findings as **proper child work items** with type and severity, linked by parent FK to the originating bead's parent epic (per `feedback_findings_epic_parent`).

| Field | Values |
|---|---|
| `type` | `finding` |
| `severity` | `critical`, `major`, `minor`, `risk`, `extra`, `deviation` |
| `parent_id` | Originating bead's parent epic |
| `discovered_from_id` | Originating bead (the work item that surfaced this finding) |
| `kind_of_finding` | `review` or `qa` (which gate produced it) |

This unlocks first-class queries the v1 label-based approach could not express:

- "All `severity=critical` findings across this org, regardless of project"
- "All findings discovered from work items in iteration X"
- "Findings closed without a corresponding `pr` comment" (audit)
- "Median time-to-close per severity"

### 6.7 Pipeline state machine

Authoritative state transitions enforced at the MCP boundary (FR-14, Law 8 layer 1). A transition is rejected with a structured error if the precondition does not hold.

| From | To | Precondition |
|---|---|---|
| (any) | `claim` | `Status == Ready` and `claimed_by_id IS NULL` |
| `impl_state=pending` | `impl_state=done` | `claimed_by_id IS NOT NULL`; comment trail includes `kind=completed` |
| `review_state=pending` | `review_state=approved` | `impl_state=done`; comment trail includes `kind=review, status=success` |
| `review_state=pending` | `review_state=needs_rework` | `impl_state=done`; comment trail includes `kind=review, status=warning` |
| `qa_state=pending` | `qa_state=passed` | `review_state=approved`; comment trail includes `kind=qa, status=success` |
| `qa_state=pending` | `qa_state=failed` | `review_state=approved`; comment trail includes `kind=qa, status=error` |
| `qa_state=failed` | `qa_state=passed` | **Override path** — comment trail includes `kind=override, status=warning` with `body` length ≥ 20 chars; user-confirmed via Quinn (§6.11). Sets `is_override=true` on the qa-state event audit |
| `pipeline_state=running` | `pipeline_state=needs_human` | 3× `review_state=needs_rework` on the same item OR 3× `qa_state=failed` OR claim conflict OR worktree conflict OR explicit `mcp.flag_human` call |
| `Status=*` | `Status=Done` | `qa_state=passed` (which is reached either via the qa PASS row above, or via the override path row above) |

The state machine is a function of the three dimensions plus `pipeline_state`. Transitions are exposed as MCP tool calls (`set_state`, `claim`, `close`); the agent prompt structure (FR-16) carries the same preconditions as explicit BLOCK conditions; the post-dispatch validator (FR-15) re-validates after every dispatch.

### 6.8 Personas and supervisors (mister-anderson catalogue)

#### 8 fixed personas (workflow-level, stage-bound)

| Persona | Role | Model | Memory integration |
|---|---|---|---|
| **Grace** | Product Manager (Stage 1: manifesto, requirements) | opus | Reads `org` / `project` memory for prior product decisions; writes summarised PM decisions back |
| **Ada** | Architect + Coherence Reviewer (Stage 1 architecture, Stage 2 plan / spec) | opus | Reads memory for architectural conventions and prior phase learnings; writes phase-level architectural decisions |
| **Smith** | Research / API validator (Stage 2 research) | opus | Reads memory for prior research findings; writes validated API / library pin decisions |
| **Sherlock** | Investigator (Stage 3 investigate) | opus | Reads memory for project conventions before investigation; writes summarised findings as scoped facts |
| **Fernando** | Issue Owner (Stage 2 tasks, Stage 3 finding tracking) | sonnet | Writes finding work items linked to memory entries when patterns recur |
| **Linus** | Code Reviewer (Stage 3 review) | opus | Reads memory for review patterns and team conventions; writes recurring code-review patterns as facts |
| **Quinn** | QA Gate (Stage 3 quality) | opus | Reads memory for QA patterns; writes QA insights and recurring failure modes as facts |
| **Daphne** | Discovery / supervisor installer (Ops) | sonnet | Reads memory for prior stack-detection results to skip redundant probing |

Dropped from the v1 catalogue: Martin (refactorer — collapsed into supervisors), Gadget (inspector — collapsed into Quinn / pipeline enforcement layer 2).

#### Dynamic supervisors (stack-bound, dispatched by `/do`)

`://unblock`'s active set:

| Persona | Stack | Detection signal |
|---|---|---|
| **Greta** | Go (Encore) | `apps/api/encore.app` + Go modules |
| **Aria** | TypeScript / Astro / line-ui | `apps/web/astro.config.*` + line-ui imports |
| **Neo** | Rust | `crates/Cargo.toml` (covers both `unblock-code` and `unblock-plugin`) |
| **Olive** | Infrastructure / CI-CD | `.github/workflows/*.yml`, Encore deploy config, Cloudflare Pages config |

Other dynamic supervisors from the m-a v1 catalogue (Nina, Luna, Violet, Tessa, Juno, Kali, Maya, Isla, Ava, Nova, Iris) remain available — Daphne provisions them via `/add-supervisor` when a future project uses those stacks. The catalogue is open; specific supervisors are activated only when their detection signal fires.

### 6.9 Skills catalogue

20 user-invocable skills + 1 shared-only. Each skill has a stage tag for the Copilot-facing description-contract lint.

| # | Slash | Stage | Persona / actor | Memory integration |
|---|---|---|---|---|
| 1 | `workflow` | Meta | meta-orchestrator | — |
| 2 | `setup` | Ops | Daphne | Reads stack-detection memory; writes detection result |
| 3 | `add-supervisor` | Ops | Daphne | — |
| 4 | `product` | 1 | Grace + Ada (orchestrator) | — |
| 5 | `manifesto` | 1 | Grace | — |
| 6 | `requirements` | 1 | Grace | Reads `project` / `org` memory for prior PRD decisions |
| 7 | `architecture` | 1 | Ada | Reads memory for architectural conventions; writes high-level decisions |
| 8 | `specification` | 2 | Ada + Smith + Fernando (orchestrator) | — |
| 9 | `plan` | 2 | Ada | Reads memory for prior phase learnings; writes phase-level plan rationale |
| 10 | `research` | 2 | Smith | Reads memory for prior research; writes validated assumptions |
| 11 | `spec` | 2 | Ada | Reads + writes architectural decisions |
| 12 | `tasks` | 2 | Fernando | — |
| 13 | `implementation` | 3 | Supervisor + Sherlock + Linus + Quinn + Fernando (orchestrator) | — |
| 14 | `investigate` | 3 | Sherlock | Reads memory for project conventions; writes summarised findings |
| 15 | `do` | 3 | Supervisor (dynamic) | Reads memory for stack conventions and prior decisions |
| 16 | `review` | 3 | Linus + Fernando | Reads memory for review patterns; writes recurring patterns |
| 17 | `quality` | 3 | Quinn + Fernando | Reads memory for QA patterns; writes QA insights |
| 18 | `update` | Ops | Fernando | — |
| 19 | `reconcile` | Ops | MCP | Queries audit trail; surfaces drift hints from memory |
| 20 | `doctor` | Ops | MCP | Queries memory for context-driven health hints |

**Shared-only (not user-invocable):** `subagents-discipline`.

**Description contract** (Copilot-facing lint, enforced by `unblock-plugin`'s `build.rs`): every slash skill's description must start with an imperative verb, name the input object, include a trigger phrase, and end with a stage tag `[product] | [spec] | [impl] | [ops]`.

### 6.10 Severity thresholds for review and QA findings

The severity of a finding determines whether it forces rework, becomes its own work item, or is batched into a cleanup item.

| Gate | Severity | Action |
|---|---|---|
| Review | `CRITICAL` | Forces rework on the originating bead; **never** produces a separate finding item |
| Review | `WARNING` | Individual finding work item, `type=finding, severity=major`, sub-issue of the originating bead's parent epic |
| Review | `SUGGESTION` | Batched cleanup finding (`severity=minor`) per epic, or per-severity finding items |
| QA | `BLOCKER` | Forces rework; never produces a separate finding item |
| QA | `MAJOR` | Individual finding item, `severity=major` |
| QA | `MINOR` | Batched cleanup finding, `severity=minor` |
| QA | `RISK` | Individual finding item, `severity=risk` |
| QA | `DEVIATION` | Individual finding item, `severity=deviation` |
| QA | `EXTRA` | Individual finding item, `severity=extra` |

Findings always live in the same parent epic as the bead that originated them — there is **no separate "Review Findings" epic** (per `feedback_findings_epic_parent`).

### 6.11 Rework paths

#### Review NEEDS-REWORK

1. Linus writes a `kind=review, status=warning` comment + CRITICAL / WARNING findings (CRITICAL inline, WARNING as finding items).
2. MCP `set_state(review_state=needs_rework)` → resets `qa_state=pending` atomically.
3. Auto-dispatch supervisor for rework via `/do`.
4. New cycle: `kind=decision` / `deviation` → `kind=completed` → new `kind=review`.
5. **Escape valve:** 3× NEEDS-REWORK on the same bead → `pipeline_state=needs_human`.

#### QA FAIL — three sub-options

| Option | Effect |
|---|---|
| **rework** | Returns to supervisor; full cycle re-implementation + re-review + re-QA |
| **follow-up** | Fernando creates finding items under the parent epic; the original bead proceeds to Done (degraded, with a `kind=qa, status=warning` comment marking the deferred concerns) |
| **override** | User confirms with reason ≥ 20 chars; Quinn writes a `kind=override, status=warning` comment; `set_state(qa_state=passed, override=true)`; Fernando creates a `severity=risk` finding item to track the bypassed condition |

### 6.12 Plugin hooks

| Hook | Purpose | Claude Code mapping | Copilot cloud mapping |
|---|---|---|---|
| `session-start` | Dashboard + MCP `prime` call; load org / project memory; surface ready set | `SessionStart` event | `sessionStart` event |
| `inject-discipline-reminder` | Pre-dispatch reminder — supervisor disposition rules, BLOCK conditions | `PreToolUse` matcher = `Task` | `preToolUse` filter = sub-agent |
| `verify-state` | Post-stop validation — MCP `verify_can_transition` ensures the dispatched agent did not violate the state machine | `Stop` event | `agentStop` event |

Copilot local: zero hooks (no programmable hook surface). The dispatch convention (`@<persona>: <task>`) substitutes for explicit hooks.

### 6.13 Happy path — work item lifecycle

| # | Action | Comment | State change | `pipeline_state` |
|---|---|---|---|---|
| 1 | Item created | — | `Status=Backlog`, `impl_state=pending` | `running` |
| 2 | Becomes ready (no open blockers) | — | `Status=Ready`, `is_ready=true` (materialised) | `running` |
| 3 | Supervisor `claim` | — | `claimed_by_id` set, `Status=InProgress` | `running` |
| 4 | Investigation (optional) | `kind=investigation, status=info` | — | `running` |
| 5 | Design decisions (N×) | `kind=decision, status=info` | — | `running` |
| 6 | Plan deviations (N×, optional) | `kind=deviation, status=warning` | — | `running` |
| 7 | Implementation complete | `kind=completed, status=success` | `impl_state=done` | `running` |
| 8 | Review APPROVE | `kind=review, status=success` | `review_state=approved` | `running` |
| 9 | QA PASS | `kind=qa, status=success` | `qa_state=passed` | `running` |
| 10 | PR opened | `kind=pr, status=info` | — | `running` |
| 11 | PR merged | `kind=pr, status=success` | — | `running` |
| 12 | Item closed (Fernando via `/update`) | — | `Status=Done`, `closed_at` set | `running` |

**Close semantics**: closing the item does not delete or relabel anything. Findings linked via `discovered_from_id` remain queryable. The full audit trail (state changes, comments, claimed-by history) is preserved in Postgres indefinitely.

---

## 7. Non-Functional Requirements

- **NFR-1 — Latency.** `prime → ready → claim` p99 < 2 s warm cache (Law 7). Measured end-to-end from MCP call ingress to response egress; warm cache means a Postgres connection pool already established and the agent already authenticated.
- **NFR-2 — Tenant isolation.** Zero cross-tenant RBAC leaks. Enforced by an exhaustive security regression suite that runs against every release candidate; any failure is release-blocking.
- **NFR-3 — Source-of-truth durability.** Postgres is the canonical store (Law 3). Provider outages must not stop the product from operating; reconciliation runs on a schedule.
- **NFR-4 — BFF discipline.** The browser never holds backend credentials (Law 4). Astro Actions are the only privileged client; HttpOnly cookies live on the Astro origin only; the Encore API is unreachable from the browser except for the documented public endpoints (FR-12). The OAuth callback is on the Astro origin, never on Encore.
- **NFR-5 — Graph integrity.** The dependency graph is a DAG at all times; cycle creation is rejected at write time, never tolerated and lazily detected.
- **NFR-6 — Pipeline structural enforcement.** Bypassing the pipeline must require simultaneous bypass of all three layers (FR-14, FR-15, FR-16). A design that fails this property is wrong by definition (Law 8).
- **NFR-7 — Memory secret sanitisation.** Always on; no opt-out at v1. Detection is best-effort; warning is mandatory; the sanitised form is what is stored.
- **NFR-8 — AST CLI performance gates** (per `docs/code-cli/plan.md` §14, locked):
  - HARD: `find-symbol` p99 < 10 ms; `outline` p99 < 20 ms; `list-symbols` p99 < 50 ms; `search` p99 < 30 ms (medium corpus, warm cache).
  - HARD: FTS5 PRAGMA assertion fires on connection open; binary refuses operation on a SQLite without FTS5.
  - HARD: per-query mtime check verified by integration test.
  - SOFT: ROI median ≥ 2.0× vs `Glob/Grep/Read` baseline across 3 flows × N=10 runs (release-gate one-shot, not per-PR CI).
- **NFR-9 — Decoupled deliverables.** `unblock-mcp`, the Astro web client, and `unblock-code` ship independently and share no runtime state (Law 6).
- **NFR-10 — Quality gates.** Every change must pass `cargo fmt --check --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo doc --no-deps --workspace` (zero warnings) — extended analogously for the Go backend and Astro frontend.
- **NFR-11 — Coding discipline.** `#[non_exhaustive]` on growable public enums in library crates; `snafu` errors with crate-scoped `Result<T>`; no `unwrap()` / `expect()` outside tests; `#![deny(unsafe_code)]` workspace-wide.
- **NFR-12 — Logging.** `tracing` JSON Lines on STDERR; STDOUT reserved for protocol payloads (MCP envelopes, CLI JSON envelopes). Never mix progress and results.

---

## 8. Phasing

The project ships in four sequential phases plus a renderer phase. The AST CLI ships **before** the web frontend so the user can dogfood the backend via MCP + CLI before web UX is built.

### P01 — Backend MVP

- Encore Go on a single Postgres (8 schemas).
- Services: `auth`, `org`, `workitems`, `deps`, `memory`.
- Minimal MCP server with **14 tools** over Streamable HTTP (per FR-8).
- Public Encore endpoints: `POST /webhooks/github`, `POST /mcp` + `GET /mcp` (single logical MCP endpoint, Streamable HTTP per FR-12). OAuth callback is on Astro origin per FR-12.
- Exit criterion: an agent can authenticate via Bearer API key and complete `prime → ready → claim → close` against a manually-seeded graph; cascade fires; cycle detection rejects offending edges.

### P02 — Backend complete

- `providers` service: GitHub webhook ingestion + bidirectional sync.
- Remaining MCP tools, totalling **18 tools** at end of P02 (including the 4 memory tools).
- Pipeline enforcement layer 1: MCP state-transition validation per Manifesto Law 8.
- Exit criterion: a GitHub repository can be linked, webhooks normalise events into canonical work items, and an attempt to mark a work item `done` without the required comment trail is rejected at the MCP boundary.

### P03 — AST CLI v1.0.0

- Rust workspace per `docs/code-cli/plan.md` and `docs/code-cli/spec.md` verbatim.
- Three new crates (`unblock-indexer-core`, `unblock-indexer`, `unblock-code`).
- 10 statically-linked tree-sitter grammars (8 default + 2 opt-in).
- 11 CLI commands; SQLite + FTS5 + WAL local index.
- Distribution: cargo-dist + Homebrew + npm.
- Exit criterion: all 9 HARD gates in `plan.md` §14.1 pass on a fresh clone; ROI harness publishes raw run logs + per-flow medians as a release artifact.

### P04 — mister-anderson plugin renderer

- Rust binary `unblock-plugin` in the `crates/` workspace (sibling of `unblock-code`).
- Stage 3 implementation tier of Law 8: full three-layer enforcement.
- Renders the typed catalogue (8 fixed personas, dynamic supervisors, 20 skills, 3 hooks per §6.8 / §6.9 / §6.12) onto Claude Code, GitHub Copilot cloud, and GitHub Copilot local.
- Layer 2 enforcement: post-dispatch state validator runs against MCP `verify_can_transition` to ensure the dispatched session did not bypass the state machine. Implemented via the `verify-state` plugin hook (Stop / agentStop event) calling MCP — no separate inspector agent.
- Layer 3 enforcement: agent prompt structure carries explicit BLOCK conditions matching the MCP state machine.
- Exit criterion: an attempt to bypass the pipeline (e.g. mark `done` without a `kind=review, status=success` comment) is rejected by the MCP server (Layer 1, P02), the post-dispatch hook flags it (Layer 2), and the agent prompt's BLOCK condition would have refused to issue the call (Layer 3). All three layers agree.

### P05 — Astro web (ships at v1.1)

- Astro 5 + line-ui on Cloudflare Pages.
- Astro Actions BFF; HttpOnly cookie on the Astro origin.
- Views: kanban, dependency graph, roadmap (with recursive milestone tree per §6.3), per-item comments.
- Auth flow with OAuth2+PKCE to GitHub or GitLab.
- Web ships **after** v1.0 because the agent-facing surface (backend MCP + AST CLI + plugin) is fully usable without a UI. Humans dogfood via the plugin's structured workflow during the v1.0 → v1.1 window; the web is a humans-first triage and visualisation layer over a system that already runs.
- **External hard dependency:** `line-ui` (vitamin repo, websublime-internal) must reach feature-complete v1 covering at minimum: forms, dialogs, dropdowns, popovers, tabs, toasts, navigation, accordions, tooltips, date pickers, comboboxes, and mobile-responsive defaults. P05 implementation cannot begin before line-ui v1.
- Exit criterion: a developer can authenticate, see the same graph the agent sees, and act on it through the BFF without the browser ever obtaining Encore credentials.

### v1.0 launch scope

**v1.0 launches headless** — P01 + P02 + P03 + P04 ship at v1.0. The plugin (P04) is in v1.0 — not v1.x — because Manifesto Principle 8 and Law 8 mandate full three-layer pipeline enforcement at first launch.

**P05 (Astro web) ships at v1.1**, blocked on `line-ui` (vitamin repo) reaching a feature-complete v1 milestone. The agent-facing surface (backend MCP, AST CLI, plugin) is fully functional without a web UI — Manifesto Principle 4 ("three orthogonal deliverables, each useful alone") is the design constraint that makes a headless v1.0 launch coherent.

---

## 9. Out of Scope

### 9.1 Manifesto-locked out-of-scope (verbatim from `docs/MANIFESTO.md`)

- **Desktop application.** `://unblock` ships as web (Astro) + remote MCP + standalone CLI. There is no GPUI, Tauri, or Electron desktop app.
- **Code generation by the AST CLI.** `unblock-code` indexes, queries, and reports. It never writes code, refactors, or modifies source files.
- **Custom storage that duplicates Postgres.** No local SQLite caches inside the API service, no Redis-backed shadow state, no per-client serialisation. Postgres is enough.
- **Provider-specific UI.** When a work item maps to a GitHub issue, the product links to GitHub for the native experience. We do not reinvent GitHub's PR review or GitLab's merge request UI.
- **Replacing wikis, CMSs, or knowledge bases.** The `memory` service stores atomic facts, not documents. 8 KB max per entry, no rich-text editor, no hierarchy. We are not Notion or Confluence.
- **Network-level multi-tenant isolation.** RBAC is org-level row-level filtering, not VPC isolation. Enterprise SOC 2-grade isolation is explicitly post-v1.
- **Self-hosting story for v1.** The product runs on Encore Cloud and Cloudflare Pages. Self-hosting Encore + Cloudflare Workers compatible Postgres is technically possible but not supported by us in v1.
- **Real-time collaboration on work item content.** Editing a description is single-user. We are not Figma or Google Docs.
- **Agent decision-making.** `://unblock` tracks state and exposes the graph. It does not decide what an agent should work on next, how to implement a task, or how to write tests. The agent decides; the platform informs.

### 9.2 Additional v1.0 out-of-scope

- **Import tooling from `bd` / Linear / Jira / GitHub Issues.** Costly, distracts from the core graph engine; explicit out-of-scope at v1.0.
- **SLA / uptime guarantees.** The product runs on Encore Cloud's free tier at v1 scale, which carries no SLA. No SLA is offered to users.
- **Mobile native clients.** Web-first. The Astro client is mobile-responsive via line-ui defaults; native iOS/Android apps are post-v1.
- **GitLab provider integration at v1.0.** Deferred to v1.1 (separate work, fork-OAuth pattern).
- **Linear / Jira provider integration.** Manifesto says "eventually"; not v1, not v1.x — backlog with no committed phase.

---

## 10. Dependencies & Constraints

### 10.1 External services and projects

- **Encore Cloud** (free tier) — backend hosting, Pub/Sub, Postgres provisioning.
- **Cloudflare Pages** — Astro web frontend hosting (P05, v1.1).
- **GitHub** — OAuth identity, webhooks, REST / GraphQL for bidirectional sync.
- **GitLab** — OAuth identity at v1.0; full integration deferred to v1.1.
- **Anthropic API** — used only by the ROI harness for the AST CLI release-gate measurement; not a runtime dependency of the product.
- **`line-ui` / `vitamin` (websublime-internal project, repo `/Users/ramosmig/Public/WS-Labs/vitamin`)** — headless Web Components library powering the Astro web client (P05). **Hard dependency for P05** — the vitamin repo must reach feature-complete v1 before P05 implementation begins. Not external in the vendor sense, but external in the build-pipeline sense — line-ui has its own release cadence in its own repo.

### 10.2 Tech stack constraints

- **Backend:** Encore Go on a single Postgres (8 schemas). No additional persistent stores. No Redis. No local SQLite inside the API service.
- **Web (P05, v1.1):** Astro 5 + line-ui (websublime-internal headless Web Components on top of Zag.js, vitamin repo). BFF via Astro Actions; HttpOnly cookie on the Astro origin only. P05 is gated on line-ui v1 — see §10.1.
- **AST CLI:** Rust (edition 2024) workspace with `tree-sitter` + `sqlx` (sqlite + FTS5 + WAL) + `ignore` + `clap`. Statically-linked grammars; build requires a host C toolchain (gcc/clang/Apple Clang/MSVC).
- **MCP transport:** Remote MCP over **Streamable HTTP** per the MCP 2025-06-18 spec (single endpoint `/mcp`, two methods `POST` + `GET`), with `Bearer <api-key>`. Go SDK: `github.com/modelcontextprotocol/go-sdk`.

### 10.3 Architectural constraints (Manifesto Laws)

- **Law 1 — Cascade is structural.** Every close recomputes the graph and promotes via Pub/Sub.
- **Law 2 — One graph, one truth.** Postgres wins on disagreement.
- **Law 3 — Postgres is the source of truth.** Provider outages must not stop the product.
- **Law 4 — BFF is structural.** Browser holds no backend credentials.
- **Law 5 — Claim semantics are atomic.** `SELECT FOR UPDATE` transaction.
- **Law 6 — Decoupled deliverables share no runtime state.** AST CLI and backend are independent.
- **Law 7 — One command away from productive work.** `prime → ready → claim` < 2 s warm cache.
- **Law 8 — Pipeline gates are enforced architecturally.** Three independent layers, all must be bypassed simultaneously.

A design that violates any law is wrong by definition. A feature that requires relaxing a law is not built.

---

## 11. Success Metrics (PRD-level north stars)

Five north-star metrics gate v1.0. All other quantitative metrics (webhook latency, concurrent MCP clients, individual tool budgets, etc.) live in their phase plans, not the PRD.

- **M-1 — `prime → ready → claim` p99 < 2 s warm cache.** Law 7. HARD release gate.
- **M-2 — Zero cross-tenant RBAC leaks.** Enforced by an exhaustive security regression suite executed against every release candidate. HARD release gate; any failure blocks release.
- **M-3 — AST CLI ROI ≥ 2.0× vs `Glob/Grep/Read` median.** Across 3 representative agent flows × N=10 runs (per `docs/code-cli/spec.md` §15). **SOFT gate** — release-publishes the report; if median is below 2.0× on any flow, open a `unblock:finding:risk` follow-up bead. Does not block release.
- **M-4 — Pipeline completion rate without rework ≥ 70%.** Manifesto Principle 8: discipline value of the pipeline. Measured as the share of work items that complete the full pipeline (`investigation → implementation → review → QA`) without re-opening or re-claiming. Target ≥ 70% over the first 30 days post-launch.
- **M-5 — Cascade events per day.** Graph engagement metric. Counts `cascade` Pub/Sub events emitted by the backend per active org per day. Target: non-zero on the median active org from week 2 onward (i.e. the graph is actually used as a graph, not a flat list).

---

## 12. Risks

- **R-1 — Encore Cloud free-tier limits.** The product is launched on a free tier with no SLA; outages are possible. Mitigation: Postgres is the source of truth (Law 3), agents and the web client surface degraded mode, and the public endpoints are designed to fail loudly rather than silently.
- **R-2 — GitHub webhook reliability.** Webhooks can be missed (delivery failures, repo-level outages). Mitigation: scheduled reconciliation per Law 3; webhooks are an event source, never the source of truth.
- **R-3 — Pipeline enforcement bypass via direct Postgres writes.** A privileged operator could write directly to Postgres and skip MCP validation. Mitigation: the three-layer enforcement (MCP validation, post-dispatch validator, agent prompt) covers the agent path; direct DB writes are an operator-error category outside Law 8's scope.
- **R-4 — AST CLI ROI under 2.0×.** The ROI gate is SOFT precisely because the harness is expensive (Anthropic API cost, Sonnet output non-determinism) and could fail for harness reasons rather than indexer reasons. Mitigation: publish raw logs + per-flow medians as a release artifact so reviewers can assess severity; open a follow-up bead if missed.
- **R-5 — Static-linked binary size pushback.** Default install of `unblock-code` is constrained to ~30 MB stripped (research R-CLI-2); `lang-cpp` and `lang-ruby` are opt-in to land under the threshold. Mitigation: documented in plan §3 and §14.1 H2; size is a SOFT gate (S1).
- **R-6 — Memory secret sanitiser false negatives.** Best-effort detection cannot catch every credential pattern. Mitigation: warning is mandatory; sanitised form is stored; documentation flags this clearly.
- **R-7 — Single-vendor lock-in via GitHub at v1.0.** GitLab arrives at v1.1, but a v1.0 user is effectively GitHub-only. Mitigation: provider-agnostic architecture (Principle 3) ensures the canonical store is neutral; GitLab is an integration project, not a re-architecture.
- **R-8 — line-ui maturity gates P05.** websublime line-ui (vitamin repo) is a young headless component library. **P05 (Astro web) cannot start before line-ui v1 ships** — feature completeness for forms, dialogs, dropdowns, popovers, tabs, toasts, navigation, accordions, tooltips, date pickers, comboboxes, and mobile-responsive defaults. If line-ui slips, P05 slips. Mitigation: P05 is decoupled from v1.0 (ships at v1.1), so line-ui can grow at its own cadence in the vitamin repo without pressuring the v1.0 launch. Component-level replacement remains a fall-back if a specific line-ui primitive proves blocking, but the design assumption is line-ui matures first.
- **R-9 — Plugin renderer (P04) scope creep.** Rendering correct prompts onto both Claude Code and GitHub Copilot is harder than rendering onto one. Mitigation: scope is locked to "render BLOCK conditions matching the MCP state machine"; richer renderer features are post-v1.

---

## 13. Open Questions

None at PRD time. All eight discovery questions are resolved in §1–§12 and the Manifesto. Architectural-level questions (exact MCP tool names and signatures, exact Postgres DDL, exact Pub/Sub topic shape, exact OAuth scope set) are deferred to `docs/SPEC.md` (Ada — architect).

---

## 14. Appendix

### 14.1 Reference documents

- [docs/MANIFESTO.md](./MANIFESTO.md) — APPROVED 2026-05-07. Primary alignment doc; 8 principles, 8 governing laws, out-of-scope list. Non-negotiable.
- [docs/code-cli/plan.md](./code-cli/plan.md) — APPROVED. AST CLI phase plan (v1.0.0); carries forward verbatim into P03.
- [docs/code-cli/spec.md](./code-cli/spec.md) — APPROVED. AST CLI authoritative spec (v1.0.0).
- [docs/code-cli/research.md](./code-cli/research.md) — AST CLI research file (R-CLI-1 … R-CLI-5 closed).
- [bd persistent memory key](#) `unblock-architecture-locked-2026-05-07-after-iterative-design` — strategic context including the SonicJS and Connect rejections that shaped the Encore + Astro + Rust three-deliverable architecture.

### 14.2 Persona summary

| Persona | Primary surface | Pain solved |
|---|---|---|
| AI agent | MCP over Streamable HTTP | No graph traversal cost; one call to ready work; structured pipeline; scoped memory. |
| Orchestrator | MCP over Streamable HTTP + web | Dispatch by readiness, see cycle violations, observe cascades. |
| Developer | Astro web + AST CLI | Graph visualisation, comment review, fast code navigation without `Glob/Grep/Read` chains. |

### 14.3 Phase / deliverable cross-reference

| Phase | Deliverable | Ships at | Carries forward verbatim from |
|---|---|---|---|
| P01 | Backend MVP (Encore Go + Postgres + 14 MCP tools) | v1.0 | New work, `docs/SPEC.md` (architect) |
| P02 | Backend complete (providers + 18 MCP tools + Layer 1 enforcement) | v1.0 | New work, `docs/SPEC.md` (architect) |
| P03 | AST CLI v1.0.0 | v1.0 | `docs/code-cli/{plan,spec,research}.md` |
| P04 | mister-anderson plugin renderer (Layer 2 + 3) | v1.0 | New work, `docs/SPEC.md` (architect) |
| P05 | Astro web (kanban + graph + roadmap + comments) | **v1.1** (line-ui-blocked) | New work, `docs/SPEC.md` (architect) |

### 14.4 Competitive context

- **GitHub Issues / Linear / Jira** — flat issue lists with link-metadata dependencies. No computable ready set. No structured agent-first MCP surface. No pipeline enforcement.
- **`bd` (beads)** — provider-agnostic, dev-PM-style local tool. Not a runtime product; used inside this repo as a developer tool (per `feedback_bd_is_dev_tool_not_product`). `://unblock` is the GitHub-backed alternative for shipped products.
- **Notion / Confluence** — knowledge bases, not work trackers. Memory service is explicitly not a wiki replacement (see §9.1).
- **Code-search MCP servers** — typically wrap `ripgrep` or LSP. `unblock-code` differs by being one-shot CLI (no MCP, no editor registration), statically-linked across 10 grammars, and structurally decoupled from the issue tracker (Law 6).

---

**Status: APPROVED 2026-05-07.** Ready for `/architecture` (Ada — architect consumes this PRD as input to `docs/SPEC.md`).
