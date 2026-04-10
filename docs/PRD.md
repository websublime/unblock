# ://unblock — Product Requirements Document

> Version: 0.1-draft  
> Status: Working Draft  
> Companions: [MANIFESTO.md](./MANIFESTO.md) · [SPEC.md](./SPEC.md)  
> Plans: [01-mcp-foundation](./plans/01-plan-mcp-foundation.md) · [02-mcp-complete](./plans/02-plan-mcp-complete.md) · [03-mcp-production](./plans/03-plan-mcp-production.md) · 04-plugin (planned) · 05-remote-server (planned) · 06-llm-agent (planned) · 07-harness (planned)  
> Specs: [01-graph-engine](./specs/01-spec-graph-engine.md) · [02-github-client](./specs/02-spec-github-client.md) · [03-mcp-tools](./specs/03-spec-mcp-tools.md) · 04-plugin-pipeline (planned) · 05-remote-server (planned) · 06-llm-agent (planned)

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Problem Statement](#2-problem-statement)
3. [Vision](#3-vision)
4. [Personas](#4-personas)
5. [Core Principles](#5-core-principles)
6. [Rust Workspace](#6-rust-workspace)
7. [Phases Overview](#7-phases-overview)
8. [Success Metrics](#8-success-metrics)
9. [Risks & Mitigations](#9-risks--mitigations)
10. [Out of Scope](#10-out-of-scope)

---

## 1. Executive Summary

unblock is a dependency-aware task tracking system for AI agents, powered by GitHub. It is not a project management tool — it is the compute layer that GitHub Issues is missing.

unblock reads GitHub Issues, blocking relationships, and Projects V2 custom fields. It builds a dependency graph in memory, computes the ready set, and exposes agent-optimised tools via MCP protocol. The agent asks "what can I work on right now?" and gets an answer in under a second, with zero setup friction.

GitHub stores. Rust computes. Agents ask.

The product has three layers, deployed in two modes:

```
┌─────────────────────────────────────┐
│  Harness                            │  Orchestration: pre-defined workflows
│  "build feature X from scratch"     │  that compose plugin agents + skills
│  "triage and fix this bug report"   │  into coordinated multi-step sequences
├─────────────────────────────────────┤
│  Plugin (CC / Copilot / Cursor)     │  Agents + individual skills:
│  sherlock, ada, fernando, linus,    │  /think, /plan, /do, /make, /use,
│  quinn, martin, gadget              │  /info, /trail, /ship, /need, /setup
├─────────────────────────────────────┤
│  MCP Server                         │  Graph engine: ready, claim, close,
│  UNBLOCK://ready  ://claim  ://close │  cascade, deps — 17+ tools
└─────────────────────────────────────┘
```

**Local mode** — the MCP server runs as a local binary (`unblock-mcp`) over stdio. A developer runs Claude Code, Copilot, or any MCP client. The binary auto-detects the repo from git remote, builds the graph, serves tools. Zero infrastructure.

**Remote mode** — the same tools run on a server (`unblock-mcp-remote`) over Streamable HTTP. Multiple developers and agents connect to the same server. The graph cache is shared across sessions (no cold start after the first connection). GitHub webhooks invalidate the cache instantly. An autonomous LLM agent (Codestral) runs on the same server — triggered by webhooks, it performs investigation and code review without a human session.

```
┌─────────────────────────────────────────────┐
│          unblock server (remote)            │
│                                             │
│  ┌───────────────┐   ┌──────────────────┐  │
│  │  MCP tools     │   │  LLM Agent       │  │
│  │  (HTTP)        │◄──│  (Codestral)     │  │
│  └───────┬────────┘   └────────┬─────────┘  │
│          │                     │            │
│  ┌───────┴─────────────────────┴─────────┐  │
│  │  SharedGraphCache + Webhooks          │  │
│  └───────────────┬───────────────────────┘  │
└──────────────────┼──────────────────────────┘
                   │
                   ▼
              GitHub API
```

The CLI binary is `unblock-mcp`. The product brand is `://unblock`.

---

## 2. Problem Statement

### 2.1 The problem is structural, not cosmetic

AI agents can write code. They cannot manage work. The tools they use — flat issue lists, markdown plans, local databases — lack the one thing that makes project management possible: a dependency graph.

An agent without a dependency graph will start coding a task that is blocked by three others. An agent without a ready queue picks work at random. An agent without claim semantics duplicates work another agent is already doing. An agent without cascade closes a task and nothing downstream moves.

These are not edge cases. They are what happens every time an agent operates on a flat issue list — which is what every project management tool gives them today.

### 2.2 GitHub Issues is necessary but insufficient

GitHub Issues is the most widely adopted issue tracker in the world. It has labels, milestones, assignees, projects, and — since 2025 — native blocking relationships and sub-issues. It does not have:

- **A computable dependency graph.** Blocking relationships exist but no tool computes the transitive closure, the ready set, or the cascade.
- **A ready queue.** There is no API call that returns "issues with no active blockers, sorted by priority."
- **Claim semantics.** Assignment is not atomic. Two agents can start the same issue.
- **Cascade.** Closing an issue does not promote newly unblocked dependents.
- **Structured agent context.** There is no session injection, no structured comment protocol, no pipeline enforcement.

### 2.3 Existing solutions do not solve the problem

| Tool | Why it fails for agents |
|---|---|
| **GitHub Projects V2** | View layer — boards, tables, timelines. No dependency graph, no ready queue, no cascade, no claim semantics. An agent sees a flat list with custom fields. |
| **Linear** | Beautiful UI, no MCP interface, no dependency graph for agents, designed for humans who drag cards. |
| **Jira** | Linking issues does not produce a computable graph. No ready set, no cascade. REST API designed for human workflows and enterprise configurability. |
| **Markdown plans** | Unstructured, no dependency awareness, lost between sessions, no enforcement. |
| **Local databases (beads, SQLite)** | Single-developer, no team visibility, no native UI, no GitHub integration. |

unblock does not replace any of these. It computes what they cannot — the graph, the ready set, the cascade — and exposes it through MCP so that any agent can do productive work.

---

## 3. Vision

unblock is the dependency graph for GitHub Issues — the compute layer that makes AI agents productive project participants, not just code generators.

In one year, unblock is the default task management backend for AI agent workflows on GitHub. Teams that use Claude Code, Copilot, Cursor, or any MCP-capable client run `UNBLOCK://ready` and get unblocked work instead of scrolling through issue lists. The plugin's structured pipeline — investigation, implementation, review, QA — enforces the discipline that agent-driven development requires.

In three years, unblock is the standard for agent-native project management on GitHub. The remote server runs investigation and review autonomously — every issue gets context before an agent touches code, every PR gets a structured review within minutes. Multi-agent teams claim, implement, review, and cascade work with the harness orchestrating entire workflows end to end. The dependency graph is the source of truth for what can be done, what is blocked, and what will unblock next.

GitHub stores everything. unblock computes everything.

---

## 4. Personas

### 4.1 The AI Agent (Primary)

**Who:** Any MCP-compatible agent — Claude Code, Copilot, Codex, Cursor, Aider, or any custom agent. Operates in a coding session. Has a limited context window and no persistent memory between sessions.

**What they need from unblock:**
- A ready queue: "what can I work on right now?"
- Atomic claim: take ownership without collision
- Cascade: close work and see what opened up
- Session context: structured injection of current state at session start
- Structured responses: JSON, not formatted text

**How they use unblock:** `prime` → `ready` → `claim` → work → `close` → loop. Every interaction is a tool call through MCP — local (stdio) or remote (HTTP).

---

### 4.2 The Orchestrator (Secondary)

**Who:** A system or human coordinating multiple agents. Assigns work across agents, monitors progress, redirects when priorities change. Uses org-level projects to coordinate work across multiple repositories.

**What they need from unblock:**
- Visibility into agent allocation: who is working on what
- Assignment: `claim #42 --agent reviewer`
- Filtering: `ready --agent coder`, `blocked --agent reviewer`
- Cross-repo dependencies: track blocking relationships across repositories
- Statistics: throughput, bottlenecks, stale claims

**How they use unblock:** Orchestration tool calls — `claim` with agent parameter, `stats`, `list` with filters, `prime` scoped to agent. Typically connects via the remote server for shared state.

---

### 4.3 The Developer (Tertiary)

**Who:** The human who reviews agent work, creates epics, triages bugs, and makes architectural decisions. Interacts via GitHub UI — Projects boards, issue pages, `gh` CLI. Bootstraps projects with `init` + `setup`.

**What they need from unblock:**
- Visibility in GitHub: which issues are blocked, ready, claimed — visible on Projects V2 boards without leaving GitHub
- Audit trail: structured comments on every issue showing agent decisions, context, discoveries
- Override capability: reassign, reprioritise, close, reopen — the human is always in control
- Pipeline confidence: review and QA gates that agents cannot bypass

**How they use unblock:** GitHub UI for visibility + Projects boards. `/think` for exploration. `/plan` for phase planning. `/ship` for pre-merge readiness checks.

---

## 5. Core Principles

These are the product-level constraints that govern every prioritisation decision. They derive from the seven laws in the MANIFESTO and the technical decisions in the SPEC.

**Zero custom storage.** unblock stores nothing. GitHub Issues, Projects V2 fields, comments, and blocking relationships are the canonical store. If unblock disappears, your data is still in GitHub exactly where you left it. The remote server's `SharedGraphCache` is reconstructable from GitHub — it is a performance optimisation, not a source of truth.

**The graph is always recomputed.** Every write invalidates the in-memory graph and rebuilds from GitHub. There is no stale state because there is no persistent state. The cache is ephemeral. Webhooks accelerate invalidation on the remote server but do not change this invariant.

**One command away from productive work.** `prime` → `ready` → `claim` in under two seconds. If an agent needs more than one command to find work, the product has failed.

**Correct GitHub primitive.** Comments for the work log. Auto-links for references. Fields for typed data. Body sections for prose. Each data type lives where it belongs in GitHub — not duplicated in markdown or custom storage.

**Agent parity.** Every operation available through the MCP server is available to any MCP client — local or remote — with structured input and output. No human-only features. No CLI-only features.

**Same tools, two transports.** Local (`unblock-mcp`, stdio) and remote (`unblock-mcp-remote`, Streamable HTTP) execute identical tool logic from the shared `unblock-tools` crate. The transport is a deployment decision, not a feature difference.

**Session isolation is architectural.** Planning never contaminates execution. Investigation never contaminates implementation. Review never remembers writing the code. Each phase runs in an isolated session. Enforcement is structural — MCP rejects label transitions without preconditions, the inspector audits after every dispatch, prompts have explicit BLOCK conditions.

**The laws are invariants.** The seven laws in the MANIFESTO are not open for negotiation on a per-feature basis. A feature that requires relaxing a law is not built.

---

## 6. Rust Workspace

unblock is implemented as a Rust workspace that grows across phases. Each crate has a single responsibility and clean boundaries.

### 6.1 Workspace evolution

**Phases 01–03** — 3 crates (local MCP):

```
crates/
  unblock-core/              ← lib: domain types, graph engine, cache (zero network)
  unblock-github/            ← lib: GitHub API client (GraphQL + REST)
  unblock-mcp/               ← bin: MCP server binary (stdio transport)
```

**Phase 05** — 5 crates (tools extracted, remote server added):

```
crates/
  unblock-core/              ← zero changes
  unblock-github/            ← zero changes
  unblock-tools/             ← NEW lib: shared tool implementations (extracted from unblock-mcp)
  unblock-mcp/               ← becomes thin stdio bootstrap (~50 lines)
  unblock-mcp-remote/        ← NEW bin: Streamable HTTP + webhooks + SharedGraphCache (axum)
```

**Phase 06** — 6 crates (LLM agent added to the same server):

```
crates/
  unblock-core/              ← zero changes
  unblock-github/            ← zero changes
  unblock-tools/             ← zero changes
  unblock-mcp/               ← zero changes
  unblock-mcp-remote/        ← zero changes — agent is a client, not a modification
  unblock-agent/             ← NEW bin: autonomous LLM agent (Codestral, rig, webhook dispatch)
```

### 6.2 Crate dependency graph

```
unblock-mcp (bin, stdio)
  └── unblock-tools (lib)
        ├── unblock-github (lib)
        │     └── unblock-core (lib)
        └── unblock-core (lib)

unblock-mcp-remote (bin, Streamable HTTP)
  ├── unblock-tools (lib)        ← same tools, zero duplication
  └── axum + tower-http + dashmap + secrecy

unblock-agent (bin, co-deployed with remote)
  ├── rig-core                   ← LLM loop, does NOT depend on unblock-*
  ├── reqwest                    ← GitHub Contents API, PR diff
  └── HTTP client → unblock-mcp-remote  ← runtime dependency, not Rust dependency
```

### 6.3 Core dependencies

| Crate | Purpose |
|---|---|
| `petgraph` | Graph algorithms — DAG operations, cycle detection, topological sort |
| `rmcp` | Rust MCP SDK — stdio and Streamable HTTP transports |
| `reqwest` | HTTP client for GitHub API |
| `snafu` | Structured domain errors with context |
| `tracing` | Structured JSON logging to stderr |
| `tokio` | Async runtime |
| `serde` / `serde_json` | Serialisation |
| `schemars` | MCP tool schema generation |
| `axum` | HTTP server for remote MCP and agent webhooks |
| `rig-core` | Rust-native LLM agent framework (OpenAI-compatible) |

### 6.4 Error handling convention

Every crate uses `snafu` exclusively. No `thiserror`, no `anyhow`, no `Box<dyn Error>`. Every crate defines its error types in `src/errors.rs` and re-exports a crate-scoped `Result<T>` alias. `unwrap()` and `expect()` are forbidden outside of test modules.

### 6.5 Licensing

| Crate | License | Rationale |
|---|---|---|
| `unblock-core` | MIT | Open-source foundation |
| `unblock-github` | MIT | Open-source foundation |
| `unblock-tools` | MIT | Open-source — tools are the product |
| `unblock-mcp` | MIT | Open-source — local binary for everyone |
| `unblock-mcp-remote` | BSL 1.1 → MIT (4 years) | Pro/Enterprise — server infrastructure |
| `unblock-agent` | BSL 1.1 → MIT (4 years) | Pro/Enterprise — autonomous LLM agent |

---

## 7. Phases Overview

Each phase corresponds to one plan document. Phases are sequential — a phase starts when the previous one is complete. Each plan document contains the full epic and task breakdown.

---

### Phase 01 — MCP Foundation (v0.1.0) → [01-plan-mcp-foundation.md](./plans/01-plan-mcp-foundation.md)

**Status:** In Progress (11 of 17 tools implemented, 6 remaining)

The minimum viable loop. An agent can find work, claim it, create and edit issues, complete work, and see the cascade. Local binary, stdio transport.

**Scope:** 17 MCP tools — `init`, `setup`, `ready`, `claim`, `create`, `update`, `close`, `reopen`, `show`, `list`, `search`, `stats`, `prime`, `comment`, `depends`, `dep_remove`, `dep_cycles`. Cargo workspace (3 crates), CI pipeline, graph engine (petgraph), TTL cache, GitHub client (GraphQL + REST), MCP server (rmcp, stdio), `GitHubApi` trait abstraction with `MockGitHubClient`, integration tests.

**Implemented:** `init`, `setup`, `ready`, `claim`, `create`, `update`, `close`, `show`, `prime`, `comment`, `depends`. Plus `reconcile` (Phase 02 early) and agent detection (Phase 02 early).

**Remaining:** `reopen`, `list`, `search`, `stats`, `dep_remove`, `dep_cycles`. See [Plan 01 Epic 06](./plans/01-plan-mcp-foundation.md#epic-06--foundation-completion).

**Outcome:** A working local MCP server that any MCP client can connect to via stdio. The full agent workflow loop: `prime` → `ready` → `claim` → work → `close` → cascade.

---

### Phase 02 — MCP Complete (v0.2.0) → [02-plan-mcp-complete.md](./plans/02-plan-mcp-complete.md)

Production hardening and remaining MCP capabilities. Still local binary only.

**Scope:**
- `reconcile` tool — detect and repair semantic drift between graph and GitHub state (7 drift types: `StaleReadyState`, `UncascadedClosure`, `OrphanedBlockingEdge`, `MalformedAgentField`, `MissingProjectField`, `CycleDetected`, `StaleClaim`)
- `commit_context` tool — structured commit messages with git trailers for audit trail
- `doctor` tool — operational health with self-repair capability
- Circuit breaker — graceful degradation on GitHub outages (5 failures in 60s → fail fast for 10s)
- Retry with exponential backoff — 429 and 503 only (500ms base, 5s max, ±25% jitter)
- OpenTelemetry — tool duration, API duration, cache metrics, graph size
- Agent client detection — `AgentKind`, `ClientDetector`, `SessionMeta`

**Outcome:** The MCP server handles failure gracefully, detects drift from external mutations, and provides full operational observability.

---

### Phase 03 — MCP Production (v1.0.0) → [03-plan-mcp-production.md](./plans/03-plan-mcp-production.md)

Distribution, scale, and enterprise readiness. The local binary becomes installable everywhere.

**Scope:**
- Cross-platform binaries via cargo-dist — Linux x86_64/ARM64, macOS x86_64/ARM64, Windows x86_64
- Homebrew formula — `brew install websublime/tap/unblock`
- npm wrapper — `npx @unblock/cli`
- Materialised fast path (Strategy D) — Ready State field as persistent cache for cold start. Serve immediately from field, rebuild graph async. ~50-100 lines change
- GitHub Enterprise Server support — configurable `GITHUB_API_URL` and `GITHUB_URL`
- GitHub App authentication — higher rate limits (15k/h), org-wide install, bot identity

**Outcome:** `v1.0.0` release. Installable on any platform. Production-grade for teams with 500+ issues.

---

### Phase 04 — Plugin (v1.1.0) → [04-plan-plugin.md](./plans/04-plan-plugin.md)

Specialised agents and skills that turn the MCP server into a structured development pipeline. Works with the local MCP binary.

**Scope:**

10 skills — the unified entry point for Claude Code and Copilot CLI:
- **Setup** — `/setup` (bootstrap: GitHub labels, milestone, Projects V2, editor configs, hooks), `/need` (intent-based agent discovery and installation), `/doctor` (diagnostic: MCP health, GitHub state, local environment)
- **Exploration** — `/think` (free exploration — research, PRD, spec, brainstorm — no pipeline enforcement, any agent available)
- **Planning** — `/plan` (two modes: global vision defines phases in the PRD; phase planning produces both `plans/NN-plan-{slug}.md` and `specs/NN-spec-{slug}.md` — the plan defines epics and tasks, the spec defines algorithms, invariants, and edge cases. GitHub Issues are created sequentially from the plan. Both artefacts are prerequisites for implementation)
- **Execution** — `/do` (intent router: implementation, investigation, spec, spike, review, QA — routes to the right agent based on context), `/make` (autonomous execution: same routing, no human-in-the-loop, stricter preconditions)
- **Direct access** — `/use` (dispatch a specific agent by name), `/info` (natural language query over project state), `/trail` (structured narrative history of an issue)
- **Quality gates** — `/ship` (pre-merge readiness check: review passed? QA passed? dependencies closed?)

8 named agents — each a `.md` configuration file, not compiled code:

| Agent | Name | Role | Model |
|---|---|---|---|
| Investigator | Sherlock | Codebase analysis, root cause, approach | opus |
| Architect | Ada | System design, specs, phase planning | opus |
| Product Owner | Fernando | Issue creation (sequential, never batch), findings tracking | sonnet |
| Code Reviewer | Linus | Read-only review, structured findings, verdicts | opus |
| QA Gate | Quinn | Spec conformity, tests, build, lint — last gate before merge | opus |
| Refactorer | Martin | Fix validated review findings, cautious, behaviour-preserving | sonnet |
| Inspector | Gadget | Pipeline compliance — runs after every dispatch, writes AUDIT only on violations | sonnet |
| Implementation | Dynamic | Tech-specific supervisors, generated per-project by `/need` | sonnet |

3 enforcement layers — pipeline compliance is structurally impossible to bypass:
1. **MCP validation** — the server rejects label transitions when preconditions are not met. No `unblock:review:ok` without a REVIEW comment containing an APPROVE verdict. No `unblock:qa:ok` without a QA comment with a PASS verdict.
2. **Inspector (Gadget)** — runs after every agent dispatch. Verifies structured comments exist, are well-formed, and follow the correct sequence. Writes an AUDIT comment only when violations are found. Clean pipelines produce zero noise.
3. **Agent prompt structure** — numbered steps with explicit BLOCK conditions. The agent cannot proceed past a gate without the required artefact.

Session isolation via structured comment trail: INVESTIGATION → DECISION → DEVIATION → COMPLETED → REVIEW → REFACTORING → QA. Worktrees (`worktrees/issue-{N}-{slug}`) isolate implementation work. 13 labels with `unblock:` prefix. 4 hooks.

**Outcome:** A disciplined development pipeline where agents investigate, implement, review, and QA — with enforcement that is structurally impossible to bypass.

---

### Phase 05 — Remote Server (v1.2.0) → [05-plan-remote-server.md](./plans/05-plan-remote-server.md)

The same MCP tools, served over HTTP from a persistent server. Shared graph cache, webhook invalidation, multi-client support. The foundation for teams and for the autonomous agent in Phase 06.

**Scope:**

**`unblock-tools` crate** (new library) — extract all 17+ tool implementations from `unblock-mcp` into a shared library. Pure tool logic: validate input, call GitHub, rebuild graph, return result. No transport, no MCP bootstrap. Both `unblock-mcp` (stdio) and `unblock-mcp-remote` (HTTP) depend on this crate. Zero duplication.

**`unblock-mcp-remote` binary** (new) — thin HTTP bootstrap over `unblock-tools`:
- Streamable HTTP transport via rmcp + axum — single endpoint `POST /mcp`
- `SharedGraphCache` — `DashMap<CacheKey, Arc<RwLock<CacheEntry>>>` keyed by `(owner/repo, token_fingerprint)`. Graph survives between sessions. No cold start after first connection. Concurrent reads, exclusive writes per entry.
- GitHub webhook handler — `POST /webhooks/github`, HMAC-SHA256 verified. Invalidates cache on issue events (closed, reopened, opened, labeled, unlabeled). Does not trigger rebuild — next tool call rebuilds lazily.
- Auth via `Authorization: Bearer <github_token>` — token validated once per session via GitHub `/user` endpoint, cached by token fingerprint for 5 minutes.
- Repo context via MCP `initialize` handshake — client declares `unblock:repo` and `unblock:project` in meta field. Background cache warm-up dispatched immediately after initialize.
- Health endpoint — `GET /health`

**Deployment:**
- Docker image (`ghcr.io/websublime/unblock-mcp-remote`)
- GHE support — `GITHUB_API_URL` env var at deploy time, no per-request override (SSRF prevention)
- GitHub Actions integration — runners connect via HTTP config, no binary install needed

**MCP client config (remote):**
```json
{
  "mcpServers": {
    "unblock": {
      "type": "http",
      "url": "https://unblock.example.com/mcp",
      "headers": {
        "Authorization": "Bearer ${GITHUB_TOKEN}"
      }
    }
  }
}
```

**Outcome:** Teams share one server. Multiple agents and developers connect to the same graph cache. Webhooks provide instant consistency. The server is ready to host the LLM agent in Phase 06.

---

### Phase 06 — LLM Agent (v1.3.0) → [06-plan-llm-agent.md](./plans/06-plan-llm-agent.md)

Autonomous investigation and code review, running on the same server as the remote MCP. Triggered by GitHub webhooks, powered by Codestral. The agent is a **client** of the remote MCP — it calls `show` and `comment` via HTTP. It never claims, closes, or creates issues. It is read-only except for comments.

**Scope:**

**`unblock-agent` binary** (new, co-deployed with `unblock-mcp-remote`):

**Flow 1 — Autonomous Investigation (Sherlock):**
- Trigger: `issues.labeled` where label = `needs-investigation`
- Agent reads issue via MCP `show`, discovers relevant files via GitHub Contents API, calls Codestral with full context
- Produces structured `INVESTIGATION:` comment (root cause, files, approach, risks, related tests, gaps in acceptance criteria)
- Idempotent — skips if `INVESTIGATION:` comment already exists
- Removes `needs-investigation` label on success

**Flow 2 — Autonomous PR Review (Linus):**
- Trigger: `pull_request.opened` or `pull_request.ready_for_review`
- Agent extracts linked issue from PR body, reads issue + comment trail via MCP `show`, fetches PR diff via GitHub REST API
- Calls Codestral to cross-reference diff against acceptance criteria
- Produces structured `REVIEW:` comment on the Issue (verdict: APPROVE/NEEDS-REWORK, criteria check, findings)
- Submits formal GitHub PR Review with `event: "COMMENT"` (never APPROVE or REQUEST_CHANGES — the agent does not block merges)
- Idempotent — skips if `REVIEW:` comment already exists

**LLM strategy:**
- Primary: Codestral (Mistral, 22B) — code specialist, cheapest per-token, OpenAI-compatible API
- Fallback: Mistral Small 3.1 (24B) — stronger instruction following if format compliance drops
- Framework: `rig` (Rust-native LLM agent framework, tool calling, OpenAI-compatible provider)
- Cost: ~€0.001 per investigation, ~€0.0005 per review
- Self-hosted option: vLLM with Codestral for data sovereignty (GHE environments)

**Safety:**
- Max 15 LLM turns per run, max 12 files fetched, max 400 lines per file, 120s timeout
- Output format validation before posting — degraded comment on format failure
- No code write access — GitHub token scoped to Issues (R/W), Pull Requests (R/W), Contents (R)
- Agent posts comments only. Never creates branches, claims issues, or modifies code

**Auth:**
- Dedicated GitHub service account (fine-grained PAT initially, GitHub App for production — `unblock-agent[bot]` identity)
- Authenticates to `unblock-mcp-remote` via `Authorization: Bearer` like any other client
- Webhook HMAC-SHA256 verification (separate webhook subscription from remote MCP)

**Outcome:** Every issue with `needs-investigation` gets context before an agent touches code. Every PR gets a structured review within minutes of opening. The plugin's interactive Sherlock and Linus (Claude, in-session) remain unchanged — the autonomous agent is a fast first pass, not a replacement.

---

### Phase 07 — Harness (v1.4.0) → [07-plan-harness.md](./plans/07-plan-harness.md)

Orchestration layer that composes plugin agents and skills into autonomous multi-step workflows.

**Scope:**
- Pre-defined workflow templates — "build feature X from scratch", "triage and fix this bug", "plan the next sprint"
- Full pipeline automation — `/think` → `/plan` → `/do` → review → QA → `/ship`
- Agent team patterns — pipeline, fan-out, producer-reviewer, supervisor
- Multi-agent coordination — work distribution, dependency tracking, cascade handling

**Outcome:** A high-level intent produces a complete, reviewed, QA-validated implementation — without human intervention between steps. The harness does not add new capabilities. It composes what the plugin already provides.

---

## 8. Success Metrics

### Phase 01 (MCP Foundation) ✅
- `prime` → `ready` → `claim` completes in under 2 seconds on a warm cache
- Zero data loss: if unblock-mcp process dies, all state is in GitHub
- Sub-second graph rebuild for repos with <500 issues
- All 17 tools pass integration tests against live GitHub

### Phase 02 (MCP Complete)
- `reconcile` detects 100% of 7 drift types in the test corpus
- Circuit breaker activates within 60s of sustained GitHub API failure
- OpenTelemetry export produces actionable dashboards for tool latency and cache performance

### Phase 03 (MCP Production)
- Cold start under 500ms for repos with <500 issues using materialised fast path
- Cross-platform binaries pass CI on all 5 target platforms
- GHE Server integration tests pass on configurable API URL

### Phase 04 (Plugin)
- Zero pipeline violations in 100 consecutive agent dispatches (all 3 enforcement layers active)
- Session isolation: no information leaks between investigation, implementation, review, and QA sessions
- `/do` correctly routes intent to the right agent >95% of the time

### Phase 05 (Remote Server)
- Shared graph cache eliminates cold start for second+ connections to the same repo
- Webhook-triggered cache invalidation reflects GitHub changes in <1 second
- 10 concurrent MCP clients on the same server with no tool call failures

### Phase 06 (LLM Agent)
- `INVESTIGATION:` format compliance >95% of runs
- Relevant files identified (top 3 in actual implementation diff) >80% of runs
- `REVIEW:` verdict alignment with human review >75% match
- False APPROVE rate (misses a CRITICAL finding) <5%
- Per-run cost <€0.002 (investigation + review combined)

### Phase 07 (Harness)
- End-to-end "build feature" workflow completes without human intervention for well-specified features
- Harness-driven implementation passes the same review and QA gates as manual pipeline

### North star metrics
- Number of MCP tool calls per day (agent adoption)
- Number of cascade events — issues promoted to ready after close (graph value)
- Pipeline completion rate — features that pass all gates without rework (discipline value)
- Time from `ready` to `close` per issue (agent productivity)

---

## 9. Risks & Mitigations

### GitHub API rate limits constrain throughput

**Risk:** 5000 points/hour (GraphQL) + 5000 requests/hour (REST). Large cascades or frequent rebuilds could exhaust the budget.

**Mitigation:** Batch GraphQL mutations (multiple `updateProjectV2ItemFieldValue` in a single POST). Cache with TTL avoids redundant reads. Phase 03 adds GitHub App authentication for 15k/h limits. Phase 05's `SharedGraphCache` eliminates redundant rebuilds across sessions. Cascade batching collects all unblocked issues and updates fields in fewer requests.

---

### External mutations cause state drift

**Risk:** A human closes an issue via the GitHub UI. A label is changed outside unblock. The in-memory graph diverges from GitHub reality.

**Mitigation:** Every write invalidates and recomputes from GitHub (Law 2). The `reconcile` tool (Phase 02) detects and repairs 7 drift types. Phase 05's webhooks provide instant invalidation on the remote server. The graph is always rebuilt from the source of truth — drift is temporary, not persistent.

---

### Session isolation adds friction for simple tasks

**Risk:** Developers working on a quick bug fix find the investigation → implementation → review → QA pipeline too heavyweight.

**Mitigation:** Session isolation is enforced by the plugin (Layer 2), not by the MCP server (Layer 1). An agent using only the MCP server has no pipeline constraints. `/think` in the plugin has no pipeline enforcement. The pipeline is opt-in at the layer level, not mandatory at the protocol level.

---

### Plugin agent quality varies with LLM capability

**Risk:** Agents produce low-quality investigation reports or miss review findings because the underlying LLM is not strong enough.

**Mitigation:** Agent model assignments are deliberate — opus for investigation, review, and QA (where quality matters most), sonnet for product owner and refactoring (where volume and speed matter more). Model assignments are configurable. The inspector (Gadget) catches structural violations regardless of agent quality.

---

### Autonomous LLM agent produces incorrect results

**Risk:** Codestral investigation misidentifies relevant files. Codestral review false-approves a broken implementation.

**Mitigation:** The autonomous agent (Phase 06) is a **first pass**, not the canonical gate. The plugin's interactive review (Linus via Claude/opus in-session) remains the gating review. The autonomous review uses `event: "COMMENT"` in the PR review API — it never APPROVE or REQUEST_CHANGES. Quality gates: format compliance >95%, file relevance >80%, verdict alignment >75%. If metrics drop, the remediation path is: improve system prompt → evaluate Mistral Small 3.1 → fine-tune on accumulated comment history.

---

### Harness automation runs without human oversight

**Risk:** An end-to-end harness workflow produces code that passes all gates but is subtly wrong.

**Mitigation:** The harness is the last phase for a reason. By the time it ships, the plugin's enforcement layers have been battle-tested. The `/ship` gate provides a final human checkpoint. The harness never merges — it produces a branch that a human can review before merge.

---

### Remote server becomes a single point of failure

**Risk:** If the remote server goes down, all connected agents lose MCP access.

**Mitigation:** The local binary (`unblock-mcp`, stdio) remains fully functional and independent. Any developer can fall back to local mode instantly — the tools are identical. The remote server adds convenience (shared cache, webhooks, LLM agent), not capability. Health endpoint enables monitoring and auto-restart.

---

## 10. Out of Scope

The following are explicitly not in scope for unblock at any phase:

- **Custom storage.** unblock does not persist data. No database, no YAML files, no local SQLite. GitHub is the only store. The `SharedGraphCache` is reconstructable from GitHub. This is Law 1.
- **Offline operation.** GitHub API access is required. unblock is not designed for disconnected use.
- **Agent decision-making.** unblock tracks state — ready, blocked, claimed, cascaded. It does not decide what an agent should work on or how to implement it. The agent decides. unblock informs.
- **Non-GitHub backends.** unblock reads and writes GitHub. It does not support Jira, Linear, GitLab, or any other issue tracker. This is by design, not a limitation.
- **Custom UI.** GitHub Issues and Projects V2 are the UI. The pre-configured views (`𝍄 UNBLOCK://ready`, `𝍄 UNBLOCK://team`, `𝍄 UNBLOCK://pipeline`, `𝍄 UNBLOCK://roadmap`, `𝍄 UNBLOCK://timeline`) provide opinionated board layouts. unblock does not build its own interface.
- **Billing, auth, or user management.** Delegated entirely to GitHub. The authentication token is GitHub's. The permissions model is GitHub's.
- **Desktop application.** unblock operates through MCP (agents) and GitHub UI (humans). There is no standalone desktop application.
- **Code generation by the autonomous agent.** The LLM agent (Phase 06) investigates and reviews. It never writes code, creates branches, or pushes commits. Implementation is the plugin's domain.
- **`anyhow` or `thiserror`.** Error handling uses `snafu` exclusively across the entire workspace.

---

*This document defines what unblock is and why. The how is in the SPEC. The when and detail is in the phase plans.*
