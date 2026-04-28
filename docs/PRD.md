# ://unblock — Product Requirements Document

> Version: 0.1-draft  
> Status: Working Draft  
> Companions: [MANIFESTO.md](./MANIFESTO.md) · [SPEC.md](./SPEC.md)  
> Plans: [01-mcp-foundation](./plans/01-plan-mcp-foundation.md) · [02-mcp-complete](./plans/02-plan-mcp-complete.md) · [03-code-indexer](./plans/03-plan-code-indexer.md) · [04-mcp-production](./plans/04-plan-mcp-production.md) · 05-plugin (planned) · 06-remote-server (planned) · 07-llm-agent (planned) · 08-harness (planned)  
> Specs: [01-mcp-foundation](./specs/01-spec-mcp-foundation.md) · 02-mcp-complete (planned) · 03-code-indexer (planned) · 04-mcp-production (planned) · 05-plugin-pipeline (planned) · 06-remote-server (planned) · 07-llm-agent (planned)

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

GitHub Issues is the most widely adopted issue tracker in the world. It has labels, milestones, assignees, projects, issue types, and — since 2025 — native blocking relationships and sub-issues. It does not have:

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

**Phase 01** — 3 crates (local MCP, minimum viable loop):

```
crates/
  unblock-core/              ← lib: domain types, graph engine, cache (zero network)
  unblock-github/            ← lib: GitHub API client (GraphQL + REST)
  unblock-mcp/               ← bin: MCP server binary (stdio transport)
```

**Phase 02** — 4 crates (resilience layer extracted to a neutral home):

```
crates/
  unblock-core/              ← unchanged (gains DriftKind::StaleStatus + #[non_exhaustive])
  unblock-github/            ← consumes unblock-resilience for HTTP breaker + retry
  unblock-resilience/        ← NEW lib: circuit breaker + retry policy, no unblock deps
  unblock-mcp/               ← adds doctor / commit_context tools, ServerMetrics
```

`unblock-resilience` is extracted in Phase 02 (not deferred) because Phase 03's grammar fetcher (in `unblock-indexer`) consumes it directly. Forcing `unblock-indexer` (code domain) to depend on `unblock-github` (issue domain) merely to share an HTTP resilience policy would couple two architecturally orthogonal product surfaces. See [02-plan-mcp-complete §6.2](./plans/02-plan-mcp-complete.md#62-reuse-mechanism--locked-extracted-unblock-resilience-crate).

**Phase 03** — 6 crates (code indexer added; same MCP binary serves both tool sets):

```
crates/
  unblock-core/              ← zero changes
  unblock-github/            ← zero changes
  unblock-resilience/        ← zero changes — consumed directly by unblock-indexer
  unblock-indexer-core/      ← NEW lib: pure indexer types, AST traversal, schema constants
  unblock-indexer/           ← NEW lib: sqlx + FTS5, tree-sitter WASM runtime, walker, watcher
  unblock-mcp/               ← adds indexer tool handlers (9 tools) alongside issue-graph tools
```

**Phases 04–05** — 6 crates (production distribution + plugin; no new crates).

**Phase 06** — 8 crates (tools extracted, remote server added):

```
crates/
  unblock-core/              ← zero changes
  unblock-github/            ← zero changes
  unblock-resilience/        ← zero changes
  unblock-indexer-core/      ← zero changes
  unblock-indexer/           ← zero changes
  unblock-tools/             ← NEW lib: shared tool implementations (extracted from unblock-mcp)
  unblock-mcp/               ← becomes thin stdio bootstrap (~50 lines)
  unblock-mcp-remote/        ← NEW bin: Streamable HTTP + webhooks + SharedGraphCache (axum)
```

**Phase 07** — 9 crates (LLM agent added to the same server):

```
crates/
  unblock-core/              ← zero changes
  unblock-github/            ← zero changes
  unblock-resilience/        ← zero changes
  unblock-indexer-core/      ← zero changes
  unblock-indexer/           ← zero changes
  unblock-tools/             ← zero changes
  unblock-mcp/               ← zero changes
  unblock-mcp-remote/        ← zero changes — agent is a client, not a modification
  unblock-agent/             ← NEW bin: autonomous LLM agent (Codestral, rig, webhook dispatch)
```

### 6.2 Crate dependency graph

```
unblock-mcp (bin, stdio)
  ├── unblock-tools (lib)        ← issue-graph tool set (Phase 06+)
  │     ├── unblock-github (lib)
  │     │     ├── unblock-resilience (lib)   ← Phase 02+
  │     │     └── unblock-core (lib)
  │     └── unblock-core (lib)
  └── unblock-indexer (lib)      ← code-indexer tool set (Phase 03+)
        ├── unblock-resilience (lib)         ← Phase 03 grammar fetcher reuse
        └── unblock-indexer-core (lib)

unblock-resilience (lib)         ← Phase 02 — no deps on other unblock crates

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
| `tree-sitter` | Multi-language parser core for the code indexer |
| `sqlx` | SQLite + FTS5 storage for the code indexer (with `sqlite` feature) |
| `ignore` | gitignore-aware filesystem walker (same as ripgrep) |
| `notify-debouncer-full` | File watcher driving incremental indexer updates |
| `rayon` | CPU-bound parallelism for indexer bootstrap |
| `failsafe` | Circuit breaker for HTTP resilience (Phase 02+ in `unblock-resilience`) |
| `backoff` | Exponential retry with jitter (Phase 02+ in `unblock-resilience`) |
| `hdrhistogram` | Latency histograms in `ServerMetrics` (Phase 02+) |

### 6.4 Error handling convention

Every crate uses `snafu` exclusively. No `thiserror`, no `anyhow`, no `Box<dyn Error>`. Every crate defines its error types in `src/errors.rs` and re-exports a crate-scoped `Result<T>` alias. `unwrap()` and `expect()` are forbidden outside of test modules.

### 6.5 Licensing

| Crate | License | Rationale |
|---|---|---|
| `unblock-core` | MIT | Open-source foundation |
| `unblock-github` | MIT | Open-source foundation |
| `unblock-resilience` | MIT | Open-source foundation — generic HTTP resilience policy |
| `unblock-indexer-core` | MIT | Open-source foundation |
| `unblock-indexer` | MIT | Open-source — code indexer is part of the product |
| `unblock-tools` | MIT | Open-source — tools are the product |
| `unblock-mcp` | MIT | Open-source — local binary for everyone |
| `unblock-mcp-remote` | BSL 1.1 → MIT (4 years) | Pro/Enterprise — server infrastructure |
| `unblock-agent` | BSL 1.1 → MIT (4 years) | Pro/Enterprise — autonomous LLM agent |

---

## 7. Phases Overview

Each phase corresponds to one plan document. Phases are sequential — a phase starts when the previous one is complete. Each plan document contains the full epic and task breakdown.

---

### Phase 01 — MCP Foundation (v0.1.0) → [01-plan-mcp-foundation.md](./plans/01-plan-mcp-foundation.md)

**Status:** Complete (per bd — bd is the source of truth for execution status).

The minimum viable loop. An agent can find work, claim it, create and edit issues, complete work, and see the cascade. Local binary, stdio transport.

**Scope:** 17 MCP tools — `init`, `setup`, `ready`, `claim`, `create`, `update`, `close`, `reopen`, `show`, `list`, `search`, `stats`, `prime`, `comment`, `depends`, `dep_remove`, `dep_cycles`. Cargo workspace (3 crates), CI pipeline, graph engine (petgraph), TTL cache, GitHub client (GraphQL + REST), MCP server (rmcp, stdio), `GitHubApi` trait abstraction with `MockGitHubClient`, integration tests.

**Outcome:** A working local MCP server that any MCP client can connect to via stdio. The full agent workflow loop: `prime` → `ready` → `claim` → work → `close` → cascade.

---

### Phase 02 — MCP Complete (v0.2.0) → [02-plan-mcp-complete.md](./plans/02-plan-mcp-complete.md)

Production hardening and remaining MCP capabilities. Still local binary only.

**Scope:**
- `reconcile` tool — detect and repair semantic drift between graph and GitHub state. 7 drift types total — Phase 01 shipped 6 (`UncascadedClosure`, `OrphanedBlockingEdge`, `MalformedAgentField`, `MissingProjectField`, `CycleDetected`, `StaleClaim`); Phase 02 adds the 7th (`StaleStatus`) to bring `ReconcileEngine` to completeness.
- `commit_context` tool — structured commit messages with git trailers for audit trail. **BREAKING CHANGE** to commit convention: a new git-trailers footer is added on top of any existing subject-line conventions. 5 canonical trailers shipped by Phase 02 (`Closes`, `Refs`, `Spec`, `Plan`, `Phase`), all derived from the agent's active GitHub issue claim and repo config — no external tracker integration required. Pre-production stance permits the break. Trailer vocabulary is extensible — future phases (esp. Phase 07 LLM Agent) may add new keys without touching the canonical set; the parser round-trips unknown trailers unchanged.
- `doctor` tool — operational health with self-repair capability. Read-only by default; `--fix` delegates to `setup`/`reconcile`; `--with-drift` opts into drift detection.
- Circuit breaker — graceful degradation on GitHub outages (5 consecutive failures → fail fast for 10s, reset on success). Built on the `failsafe` crate.
- Retry with exponential backoff — 429 and 503 only (500ms base, 5s max, ±25% jitter). Hybrid limit: 5 attempts OR 30s deadline, env-configurable. Built on the `backoff` crate.
- New crate `unblock-resilience` — circuit breaker + retry policy extracted to a neutral home so Phase 03's grammar fetcher (`unblock-indexer`) can consume it without depending on `unblock-github`. See [02-plan-mcp-complete §6.2](./plans/02-plan-mcp-complete.md#62-reuse-mechanism--locked-extracted-unblock-resilience-crate).
- In-memory `ServerMetrics` — atomic counters + `hdrhistogram` latency histograms. Captures tool durations, API durations, cache hits/misses, graph size. Exposed via the `doctor` tool's `metrics_snapshot` field.
- Agent client detection — `AgentKind`, `ClientDetector`, `SessionMeta` (already implemented during Phase 01).

**Deferred:**
- **OpenTelemetry exporter — deferred to Phase 06** (alongside the remote server). Phase 02's in-memory `ServerMetrics` is forward-compatible: the Phase 06 OTel adapter wraps the same struct without redesigning it.

**Outcome:** The MCP server handles failure gracefully, detects drift from external mutations across all 7 drift types, exposes its own health via `doctor`, and ships a rich commit-message convention that downstream phases (Plugin §7.5, LLM Agent Phase 07) can rely on.

---

### Phase 03 — Code Indexer MCP (v1.0.0) → [03-plan-code-indexer.md](./plans/03-plan-code-indexer.md)

Token-saving for AI agents. Instead of agents wasting tokens on Glob/Grep/Read to find symbols, definitions, and code structure, an embedded multi-language code indexer answers "where is X / what does Y export / show me Z" via fast structured MCP tool calls served from the same `unblock-mcp` binary as the issue-graph tool set.

Slots after Phase 02 to leverage the `unblock-resilience` crate (circuit breaker + retry) for the HTTP grammar fetch. OpenTelemetry export is deferred to Phase 06; Phase 03 instruments via the same in-memory `ServerMetrics` introduced in Phase 02.

**Scope:**
- Two new crates: `unblock-indexer-core` (pure: domain types, AST traversal, schema constants) + `unblock-indexer` (impure: sqlx + FTS5, tree-sitter WASM runtime, grammar fetcher, file walker via `ignore`, file watcher via `notify-debouncer-full`).
- 9 new MCP tools — `find_symbol`, `list_symbols`, `outline`, `get_symbol`, `search_text`, `find_references` (HEURISTIC), `list_languages`, `index_status`, `reindex`.
- Pluggable from MVP day 1 — tree-sitter WASM grammars fetched at runtime from a versioned GitHub Release of `unblock-mcp`, integrity-verified, cached under `~/.cache/unblock/grammars/`.
- Top-10 initial languages: Rust, TypeScript, JavaScript, Python, Go, Java, C, C++, Ruby, PHP. PR-driven expansion via the CI grammar matrix.
- Persisted SQLite + FTS5 index per repo under `~/.cache/unblock/repos/<repo-hash>/index.db`. WAL mode. Span-only — no body text stored.
- Pre-warm via `notify-debouncer-full` watcher + per-query mtime check + parallel bootstrap (`rayon`) in a single transaction.
- `unblock-mcp setup` extends to register the indexer tool set alongside the issue-graph tool set in Claude Desktop / Claude Code / Cursor / Zed / VS Code / JetBrains — single command.

**Out of scope (explicit non-goals):** dead-code analysis, cyclomatic complexity, redundancy/similarity detection, refactor suggestions, cross-file semantic resolution / type inference, issue/code correlation queries.

**Outcome:** `v1.0.0` release. A single MCP binary that serves both the issue-graph and the code-indexer. Agents stop spending tokens grepping the workspace for symbols.

---

### Phase 04 — MCP Production (v1.1.0) → [04-plan-mcp-production.md](./plans/04-plan-mcp-production.md)

Distribution, scale, and enterprise readiness. The local binary becomes installable everywhere.

**Scope:**
- Cross-platform binaries via cargo-dist — Linux x86_64/ARM64, macOS x86_64/ARM64, Windows x86_64
- Homebrew formula — `brew install websublime/tap/unblock`
- npm wrapper — `npx @unblock/cli`
- Materialised fast path (Strategy D) — Status field as persistent cache for cold start. Serve immediately from field, rebuild graph async. ~50-100 lines change
- GitHub Enterprise Server support — configurable `GITHUB_API_URL` and `GITHUB_URL`
- GitHub App authentication — higher rate limits (15k/h), org-wide install, bot identity

**Outcome:** `v1.1.0` release. Installable on any platform. Production-grade for teams with 500+ issues.

---

### Phase 05 — Plugin (v1.2.0) → [05-plan-plugin.md](./plans/05-plan-plugin.md)

Typed Rust crate `unblock-plugin` that renders the mister-anderson workflow onto Claude Code and Copilot (local + cloud), backed by `unblock-mcp`. `/setup` is the single entry point; the data model is authoritative; renderers emit per-target artefacts.

**Non-goals.** Remote MCP transport (Phase 06), HTTP server, webhooks, desktop UI, npm packaging.

#### 7.5.1 Baseline

Mirrors `mister-anderson` v0.3.0. Deviations explicit in §7.5.7.

#### 7.5.2 Targets

| Target | Manifest | Agents | Skills | Hooks | MCP config |
|---|---|---|---|---|---|
| Claude Code | `.claude-plugin/plugin.json` + `marketplace.json` + `CLAUDE.md` | `.claude/agents/*.md` | `.claude/skills/<n>/SKILL.md` | `.claude/hooks/` (3) | `.claude/settings.json` |
| Copilot cloud | `.github/copilot-instructions.md` | `.github/agents/*.md` | `.claude/skills/<n>/SKILL.md` (unified) | `.github/hooks/*.json` (3) | GitHub UI — chat guide |
| Copilot local | `.github/copilot-instructions.md` | — | — | — | VS Code UI — chat guide |

Skills directory unified at `.claude/skills/` — Copilot cloud reads both `.claude/skills` and `.github/skills`. Sub-agent dispatch: `Task()` on Claude Code, natural language on Copilot cloud.

#### 7.5.3 Fixed agents (8)

| Persona | Role | Model | Hooks |
|---|---|---|---|
| Grace | Product Manager | opus | — |
| Ada | Architect + Coherence Reviewer | opus | — |
| Smith | Research / API validator | opus | — |
| Sherlock | Investigator | opus | — |
| Fernando | Issue Owner | sonnet | — |
| Linus | Code Reviewer (implementation, gaps vs spec) | opus | PreToolUse(Task), Stop |
| Quinn | QA Gate (tests, spec conformance, acceptance) | opus | PreToolUse(Task), Stop |
| Daphne | Discovery / supervisor installer | sonnet | — |

Dropped from prior PRD draft: Martin, Gadget.

#### 7.5.4 Dynamic supervisors (14)

All inherit shared skills `implementation`, `do`, `workflow`, `subagents-discipline` plus hooks `PreToolUse(Task)`, `Stop`. Model: `sonnet`.

| Persona | Stack | Detection signal |
|---|---|---|
| Neo | Rust | `Cargo.toml` |
| Nina | Node backend | `package.json` + express/fastify/nest/koa |
| Luna | React | `package.json` + `react` |
| Violet | Vue | `package.json` + `vue` |
| Tessa | Python backend | `pyproject.toml` / `requirements.txt` + fastapi/django/flask |
| Greta | Go | `go.mod` |
| Juno | Java backend | `pom.xml` / `build.gradle` + Spring/Quarkus/Micronaut |
| Kali | Kotlin backend | `build.gradle.kts` + Ktor/Spring (no Android SDK) |
| Maya | Flutter | `pubspec.yaml` |
| Isla | iOS | `*.xcodeproj` / `Package.swift` |
| Ava | Android | `build.gradle` + Android SDK |
| Nova | Blockchain | `hardhat.config` / `Anchor.toml` |
| Iris | ML | `pyproject.toml` + pytorch/tensorflow/sklearn |
| Olive | Infra / CI-CD | `*.tf` / `.github/workflows/*.yml` / `k8s/` |

Daphne detects via manifest analysis **and** `docs/PRD.md` / `docs/MANIFESTO.md` / `docs/SPEC.md`. On detection failure, prompts the user for type / technology / infra.

#### 7.5.5 Skills (20 user-invocable + 1 shared-only)

| # | Slash | Stage | Persona / actor |
|---|---|---|---|
| 1 | `workflow` | Meta | (meta-orchestrator; no args ⇒ asks user) |
| 2 | `setup` | Ops | Daphne |
| 3 | `add-supervisor` | Ops | Daphne |
| 4 | `product` | 1 | (orchestrator: Grace + Ada) |
| 5 | `manifesto` | 1 | Grace |
| 6 | `requirements` | 1 | Grace |
| 7 | `architecture` | 1 | Ada |
| 8 | `specification` | 2 | (orchestrator: Ada + Smith + Fernando) |
| 9 | `plan` | 2 | Ada |
| 10 | `research` | 2 | Smith |
| 11 | `spec` | 2 | Ada |
| 12 | `tasks` | 2 | Fernando |
| 13 | `implementation` | 3 | (orchestrator: Supervisor + Sherlock + Linus + Quinn + Fernando) |
| 14 | `investigate` | 3 | Sherlock |
| 15 | `do` | 3 | Supervisor (dynamic) |
| 16 | `review` | 3 | Linus + Fernando (code-level: implementation, gaps, code review) |
| 17 | `quality` | 3 | Quinn + Fernando (output-level: tests, spec conformance, acceptance) |
| 18 | `update` | Ops | Fernando |
| 19 | `reconcile` | Ops | (MCP) |
| 20 | `doctor` | Ops | (MCP) |

**Shared-only (not user-invocable):** `subagents-discipline`.

**Description contract (Copilot-facing).** Every slash skill's `description` MUST start with an imperative verb, name the input object, include a trigger phrase, and end with a stage tag `[product] | [spec] | [impl] | [ops]`. Lint enforced in the plugin crate `build.rs`.

**`/workflow` invocation modes:**

| Form | Behaviour |
|---|---|
| `/workflow` | Show global state and **ask the user** which stage / skill to run |
| `/workflow product` | Delegate to `/product` |
| `/workflow specification` | Delegate to `/specification` (asks phase NN) |
| `/workflow implementation` | Delegate to `/implementation` (asks phase NN) |
| `/workflow next` | Auto-determine the next pending step and dispatch |
| `/workflow <skill>` | Verify prerequisites, warn, dispatch |

#### 7.5.6 Pipeline

1. **Stage 1 — Product Discovery** (`/product`) → `docs/MANIFESTO.md`, `docs/PRD.md`, `docs/SPEC.md` (project-wide, high-level).
   - `/manifesto` (Grace) → MANIFESTO; `/requirements` (Grace) → PRD; `/architecture` (Ada) → SPEC.
2. **Stage 2 — Specification** (`/specification NN`) → `docs/plans/NN-plan-*.md` + `docs/specs/NN-spec-*.md` + bead graph.
   - `/plan` (Ada) → `/research` (Smith) → `/spec` (Ada) → `/tasks` (Fernando) + Coherence Review (Ada × 3).
3. **Stage 3 — Implementation** (`/implementation NN`) — per bead: `/investigate` → `/do` → `/review` → `/quality`.

#### 7.5.7 Deviations from mister-anderson

| # | Dimension | m-a | unblock | Rationale |
|---|---|---|---|---|
| D1 | Backing store | `bd` CLI + Dolt | `unblock-mcp` tools | Phases 01–04 already shipped |
| D2 | Isolation | Git branches | Worktrees `worktrees/issue-{N}-{slug}` | User decision |
| D3 | KV store | `bd kv` | Dropped — `claim` + assignees are the source of truth | Simplification |
| D4 | State dimensions | `bd set-state` | Projects V2 custom fields | Native GitHub surface |
| D5 | Sub-agent dispatch | `Task()` everywhere | `Task()` on CC; natural language on Copilot cloud | Platform constraint |
| D6 | Mode | Local only | Local (P05); Remote deferred to P06 | Infra phasing |

#### 7.5.8 State model

Three Projects V2 single-select fields:

| Field | Values |
|---|---|
| `Unblock Impl State` | `pending`, `done` |
| `Unblock Review State` | `pending`, `approved`, `needs-rework` |
| `Unblock QA State` | `pending`, `passed`, `failed` |

State is the source of truth; the label is derived (Option X). `derive_label(impl, review, qa)`:

| impl | review | qa | Label |
|---|---|---|---|
| pending | — | — | (none) |
| done | pending | — | `unblock:review:pending` |
| done | needs-rework | — | `unblock:review:rework` |
| done | approved | pending | `unblock:review:ok` |
| done | approved | passed | `unblock:qa:ok` |
| done | approved | failed | `unblock:qa:rework` |

**Invariants:**
- `set_state` reconciles labels atomically: removes the five state-bound labels and applies the derived one.
- F2.b: writing `review=needs-rework` forces `qa=pending` in the same transaction.
- `set_state(qa=failed)` requires the current `review=approved`; otherwise rejected.
- After `qa=failed`, on the next supervisor `claim` the server performs an atomic reset `review=pending` + `qa=pending` and the label becomes `unblock:review:pending`.
- Exception labels (`unblock:needs-human`, `unblock:paused`, `unblock:no-investigation`) and finding labels (`unblock:finding:*`) are orthogonal to state dimensions and applied directly.
- Escape valve: 3 iterations of rework per gate (review OR qa) → automatic `unblock:needs-human`.

#### 7.5.9 Labels

13 labels, unchanged from SPEC §7.6 (formerly §540):

`unblock:review:pending`, `unblock:review:ok`, `unblock:review:rework`, `unblock:qa:ok`, `unblock:qa:rework`, `unblock:needs-human`, `unblock:paused`, `unblock:no-investigation`, `unblock:finding:suggestion`, `unblock:finding:minor`, `unblock:finding:risk`, `unblock:finding:deviation`, `unblock:finding:extra`.

#### 7.5.10 Crate shape

```
crates/unblock-plugin/
└── src/
    ├── lib.rs
    ├── model/          # Persona, Skill, Hook, Pipeline, Label, CommentKind, TransitionRule, Supervisor, DispatchConvention, SkillHandler
    ├── catalog/        # 8 personas, 20 skills, 3 hooks, 14 supervisors (data + templates)
    ├── detect/         # Stack detection (manifests + docs)
    ├── render/
    │   ├── claude_code.rs
    │   ├── copilot_cloud.rs
    │   └── copilot_local.rs
    └── cli.rs          # `unblock-plugin render --target=<t> --supervisors=<list> --out=<dir>`
```

Binary `unblock-plugin` invoked by `/setup` after the client choice. Description-contract lint runs in `build.rs`. Internal structure (XML vocabulary, token substitution, render contract) lives in SPEC §7.

#### 7.5.11 Hooks (3)

| Hook | Purpose | Claude Code | Copilot cloud |
|---|---|---|---|
| `session-start.sh` | Dashboard + `prime` | `SessionStart` | `sessionStart` |
| `inject-discipline-reminder.sh` | Supervisor dispatch reminder | `PreToolUse` matcher=`Task` | `preToolUse` filter=sub-agent |
| `verify-state.sh` | Enforce state dimension via `verify_agent_state` | `Stop` | `agentStop` |

m-a's `stamp-pending.sh` is dropped (KV store dropped per D3). Copilot local: zero hooks.

#### 7.5.12 `/setup` flow

| # | Actor | Action |
|---|---|---|
| 1 | `/setup` | Phase 05 supports **Local mode only**; Remote mode is deferred to Phase 06 via a dedicated bead |
| 2 | `/setup` | Collect `GITHUB_TOKEN` + `UNBLOCK_REPO` (owner/repo) |
| 3 | `/setup` | Ask client: Claude Code / Copilot |
| 4 | `/setup` | Call MCP `init` → 13 labels + milestones + Projects V2 + 3 state fields |
| 5 | Daphne | Detect stack via manifests + docs |
| 5.a | Daphne | On detection failure: ask user for type / technology / infra |
| 6 | `/setup` | Invoke `unblock-plugin render --target=<t> --supervisors=<list> --out=.` |
| 7 | Plugin binary | Write files per §7.5.13 |
| 8 | `/setup` | Claude Code: write `.claude/settings.json` with MCP stdio config |
| 8.a | `/setup` | Copilot: print **in-chat guide** with copy-pastable JSON for VS Code (local) and GitHub UI (cloud) |
| 9 | `/setup` | Summary + next steps |

#### 7.5.13 Files produced per target

| File | CC | Copilot cloud | Copilot local |
|---|---|---|---|
| `AGENTS.md` (universal workflow) | ✅ | ✅ | ✅ |
| `UNBLOCK-WORKFLOW.md` (universal MCP reference) | ✅ | ✅ | ✅ |
| `CLAUDE.md` | ✅ | — | — |
| `.github/copilot-instructions.md` | — | ✅ | ✅ |
| `.claude-plugin/plugin.json` | ✅ | — | — |
| `.claude-plugin/marketplace.json` | ✅ | — | — |
| `.claude/skills/<n>/SKILL.md` | ✅ | ✅ (unified path) | — |
| `.claude/agents/<n>.md` | ✅ | — | — |
| `.github/agents/<n>.md` | — | ✅ | — |
| `.claude/hooks/*.sh` + `hooks.json` | ✅ | — | — |
| `.github/hooks/*.json` | — | ✅ | — |
| `.claude/settings.json` | ✅ | — | — |

#### 7.5.14 Dispatch convention

`.github/copilot-instructions.md` carries an Agents table plus a section:

> Sub-agent work is delegated by name using `@<name>: <task>`. Example: `@Smith: validate API assumptions in plan 05`. The cloud agent MUST NOT execute sub-agent work inline — delegation is structural.

`CLAUDE.md` carries the same content with examples `Task(subagent_type="...")`. Both rendered from the same `DispatchConvention` struct.

#### 7.5.15 New MCP tools

| Tool | Purpose |
|---|---|
| `set_state(qualified_id, dim, value)` | Write state dim and reconcile label atomically per §7.5.8 invariants |
| `get_state(qualified_id, dim)` | Read a single state dim |
| `verify_agent_state(agent_id)` | Stop hook helper — exit 0 OK, exit 2 enforcement failure |

#### 7.5.16 Comment trail

Per-bead structured comments, in canonical order: `INVESTIGATION → DECISION → DEVIATION → COMPLETED → REVIEW → QA` plus `DEFERRED`, `PR`, `NEEDS-HUMAN`, `OVERRIDE`. Encoded as the `CommentKind` enum; `reconcile` flags out-of-order or missing kinds.

#### 7.5.17 Severity thresholds

| Severity | Gate | Action |
|---|---|---|
| CRITICAL (review) / BLOCKER (qa) | Linus / Quinn | Forces rework; never produces a finding issue |
| WARNING (review) / MAJOR (qa) | Linus / Quinn | Individual finding bead as sub-issue of the **originating bead's parent epic** |
| SUGGESTION (review) / MINOR, RISK, DEVIATES, EXTRA (qa) | Linus / Quinn | Batched or per-severity finding beads; label `unblock:finding:<kind>` |

Findings always live in the same parent epic as the bead that originated them — there is no separate "Review Findings" epic.

#### 7.5.18 Reference-only beads

The bead `design` field always points to a spec section (`docs/specs/NN-spec-foo.md#section`). Content is never inlined. Enforced by Fernando's skill template and the plugin linter.

#### 7.5.19 Deferred to Phase 06

- Remote mode in `/setup`
- Webhook-driven label / state reconciliation
- Shared cache across multiple clients

#### 7.5.20 Rework paths

**Review NEEDS-REWORK:**
1. Linus writes `REVIEW` (NEEDS-REWORK) + CRITICAL/WARNING findings.
2. `set_state(review=needs-rework)` → resets `qa=pending`.
3. Label → `unblock:review:rework`.
4. Auto-dispatch supervisor for rework.
5. New cycle: DECISION/DEVIATION → COMPLETED → new REVIEW.
6. Escape valve: 3× NEEDS-REWORK → `unblock:needs-human`.

**QA FAIL — three sub-options:**
- **rework**: returns to supervisor — full cycle re-implementation + re-review + re-QA.
- **follow-up**: Fernando creates finding beads under the parent epic; the original bead proceeds to close (degraded).
- **override**: user prompts "do override"; Quinn requests explicit confirmation + reason (≥ 20 chars); writes `OVERRIDE:` comment; `set_state(qa=passed, override=true)`; Fernando creates a `unblock:finding:risk` bead to track the bypassed condition.

#### 7.5.21 Enforcement failures

- `verify_agent_state` exit 2 — orchestrator decides re-dispatch or `unblock:needs-human`. No automatic retry.
- Label reconciliation partial — state remains authoritative; `reconcile` corrects drift on the next run.
- Claim conflicts — second `claim` returns `CLAIM_CONFLICT`; orchestrator picks another ready bead or aborts.
- Worktree conflicts — clean and same branch ⇒ reuse; dirty or different branch ⇒ `unblock:needs-human`.

#### 7.5.22 Exception modes

| Label | Source | Effect |
|---|---|---|
| `unblock:needs-human` | Auto (escape valve / conflict) or manual | Bead leaves `ready` until removed; `NEEDS-HUMAN:` comment required |
| `unblock:paused` | User | Worktree preserved; remove label to resume |
| `unblock:no-investigation` | Developer / `/plan` | Supervisor skips the investigate step |

#### 7.5.23 Setup edge cases

- **Previous setup detected** (marker in `.claude-plugin/plugin.json` or `copilot-instructions.md`) — offer update vs re-init.
- **Stack detection fails + user cancels** — abort `/setup` cleanly; no partial writes.
- **`init` MCP partial failure** — idempotent: re-run completes only what is missing.

#### 7.5.24 Happy path completion

| # | Action | Comment | State | Label |
|---|---|---|---|---|
| 1 | Bead created | — | impl=pending | (none) |
| 2 | Becomes `ready` | — | — | (none) |
| 3 | Supervisor `claim` | — | — | (none; agent in Projects V2) |
| 4 | Investigation (optional) | `INVESTIGATION` | — | (none) |
| 5 | Design decisions | `DECISION` (N×) | — | (none) |
| 6 | Plan deviations | `DEVIATION` (N×) | — | (none) |
| 7 | Implementation complete | `COMPLETED` | impl=done | `unblock:review:pending` |
| 8 | Review APPROVE | `REVIEW` (APPROVE) | review=approved | `unblock:review:ok` |
| 9 | QA PASS | `QA` (PASS / PASS+FINDINGS) | qa=passed | `unblock:qa:ok` |
| 10 | PR opened | `PR` | — | `unblock:qa:ok` |
| 11 | PR merged | — | — | `unblock:qa:ok` |
| 12 | Bead closed (Fernando via `/update`) | — | — | **all labels removed on close** |

**Close semantics.** All labels are removed on `close` — state-bound, exception, and finding labels alike. Historical record lives in comments and PR references. Epics do not auto-close when their children close; Fernando closes them explicitly via `/update`.

**Outcome:** A disciplined development pipeline where agents investigate, implement, review, and QA — with state-driven enforcement, structurally consistent across Claude Code and Copilot.

---

### Phase 06 — Remote Server (v1.3.0) → [06-plan-remote-server.md](./plans/06-plan-remote-server.md)

The same MCP tools, served over HTTP from a persistent server. Shared graph cache, webhook invalidation, multi-client support. The foundation for teams and for the autonomous agent in Phase 07.

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

**Outcome:** Teams share one server. Multiple agents and developers connect to the same graph cache. Webhooks provide instant consistency. The server is ready to host the LLM agent in Phase 07.

---

### Phase 07 — LLM Agent (v1.4.0) → [07-plan-llm-agent.md](./plans/07-plan-llm-agent.md)

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

### Phase 08 — Harness (v1.5.0) → [08-plan-harness.md](./plans/08-plan-harness.md)

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
- Circuit breaker activates after 5 consecutive GitHub API failures
- OpenTelemetry export produces actionable dashboards for tool latency and cache performance

### Phase 03 (Code Indexer)
- `find_symbol` p99 < 10ms on the medium representative repo (corpus defined by research R8)
- `outline` p99 < 20ms on the medium representative repo
- All Top-10 languages parse a mixed-language fixture repo without panic
- Grammars are fetched at runtime from the unblock-mcp GitHub Releases and integrity-verified against a signed manifest
- Adding a new language requires only a PR to the CI grammar matrix — no recompilation of `unblock-mcp`
- File watcher (`notify-debouncer-full`) keeps the index in sync on macOS, Linux, and Windows; per-query mtime check is the safety net
- `unblock-mcp setup` registers the indexer tool set alongside the issue-graph tool set in CC Desktop / CC Code / Cursor / Zed / VS Code / JetBrains under the same MCP server entry — single command
- Token-saving ROI report compares baseline (Glob/Grep/Read) vs indexer tool calls for at least 3 representative agent flows before phase close

### Phase 04 (MCP Production)
- Cold start under 500ms for repos with <500 issues using materialised fast path
- Cross-platform binaries pass CI on all 5 target platforms
- GHE Server integration tests pass on configurable API URL

### Phase 05 (Plugin)
- Zero pipeline violations in 100 consecutive agent dispatches (all 3 enforcement layers active)
- Session isolation: no information leaks between investigation, implementation, review, and QA sessions
- `/do` correctly routes intent to the right agent >95% of the time

### Phase 06 (Remote Server)
- Shared graph cache eliminates cold start for second+ connections to the same repo
- Webhook-triggered cache invalidation reflects GitHub changes in <1 second
- 10 concurrent MCP clients on the same server with no tool call failures

### Phase 07 (LLM Agent)
- `INVESTIGATION:` format compliance >95% of runs
- Relevant files identified (top 3 in actual implementation diff) >80% of runs
- `REVIEW:` verdict alignment with human review >75% match
- False APPROVE rate (misses a CRITICAL finding) <5%
- Per-run cost <€0.002 (investigation + review combined)

### Phase 08 (Harness)
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

**Mitigation:** Batch GraphQL mutations (multiple `updateProjectV2ItemFieldValue` in a single POST). Cache with TTL avoids redundant reads. Phase 04 adds GitHub App authentication for 15k/h limits. Phase 06's `SharedGraphCache` eliminates redundant rebuilds across sessions. Cascade batching collects all unblocked issues and updates fields in fewer requests.

---

### External mutations cause state drift

**Risk:** A human closes an issue via the GitHub UI. A label is changed outside unblock. The in-memory graph diverges from GitHub reality.

**Mitigation:** Every write invalidates and recomputes from GitHub (Law 2). The `reconcile` tool (Phase 02) detects and repairs 7 drift types. Phase 06's webhooks provide instant invalidation on the remote server. The graph is always rebuilt from the source of truth — drift is temporary, not persistent.

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

**Mitigation:** The autonomous agent (Phase 07) is a **first pass**, not the canonical gate. The plugin's interactive review (Linus via Claude/opus in-session) remains the gating review. The autonomous review uses `event: "COMMENT"` in the PR review API — it never APPROVE or REQUEST_CHANGES. Quality gates: format compliance >95%, file relevance >80%, verdict alignment >75%. If metrics drop, the remediation path is: improve system prompt → evaluate Mistral Small 3.1 → fine-tune on accumulated comment history.

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
- **Code generation by the autonomous agent.** The LLM agent (Phase 07) investigates and reviews. It never writes code, creates branches, or pushes commits. Implementation is the plugin's domain.
- **`anyhow` or `thiserror`.** Error handling uses `snafu` exclusively across the entire workspace.

---

*This document defines what unblock is and why. The how is in the SPEC. The when and detail is in the phase plans.*
