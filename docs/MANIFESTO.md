# ://unblock

> Dependency-aware task tracking for AI agents, powered by GitHub.  
> The work graph that agents understand.

---

## Why unblock exists

AI agents can write code. They cannot manage work.

An agent without a dependency graph will start coding a task that is blocked by three others. An agent without a ready queue picks work at random. An agent without claim semantics duplicates work another agent is already doing. An agent without cascade closes a task and nothing downstream moves.

These are not edge cases. They are what happens every time an agent operates on a flat issue list — which is what every project management tool gives them today.

GitHub Issues is the most widely adopted issue tracker in the world. It has labels, milestones, assignees, and projects. It does not have a dependency graph. It does not have a ready queue. It does not have claim semantics. It does not have cascade.

unblock does not replace GitHub Issues. unblock computes what GitHub cannot — the graph, the ready set, the cascade — and exposes it through MCP so that any agent can answer the only question that matters: **"what can I work on right now?"**

GitHub stores. Rust computes. Agents ask.

---

## What unblock believes

**GitHub is the source of truth.** Not a database. Not a YAML file. Not a local SQLite. GitHub Issues, Projects V2 fields, comments, and blocking relationships are the canonical store. unblock reads and writes GitHub. It stores nothing of its own. If unblock disappears, your data is still in GitHub exactly where you left it.

**The graph is the product.** A flat list of issues is not project management — it is a to-do list. The blocking relationships between issues form a directed acyclic graph. The ready set — issues with no active blockers — is what agents need. Everything unblock does is a function of this graph: ready, cascade, cycle detection, topological ordering.

**Compute is ephemeral.** The dependency graph lives in memory. It is rebuilt from GitHub on every session. The cache is a performance optimisation, not a source of truth. Every write invalidates and recomputes. There is no stale state because there is no persistent state.

**Agents are first-class users.** unblock was designed to be operated by AI agents via MCP. The CLI is not the primary interface — the MCP server is. Every operation is a tool call. Every response is structured. An agent that manages a team's work is not a future feature. It is the reason unblock exists.

**Claim semantics prevent collisions.** When an agent claims an issue, the claim is atomic — agent name, status change, and timestamp are set in a single operation. Two agents cannot claim the same issue. This is not a convention. It is enforced by the protocol.

**Cascade is automatic.** When an issue is closed, unblock recomputes the graph and promotes newly unblocked dependents to ready. The agent does not need to know the dependency tree. It closes its work and unblock tells it what opened up.

**The pipeline is the product.** Tools without process are chainsaws without safety guards. unblock powers a structured development pipeline — from product requirements through architecture, implementation, code review, and QA — where discipline is enforced architecturally, not by instruction alone. Three enforcement layers — MCP validation, pipeline inspector, agent prompt structure — make compliance structurally impossible to bypass.

**Sessions are sacred.** Planning never contaminates execution. Investigation never contaminates implementation. Review never remembers writing the code. Each phase runs in an isolated session. The comment trail is the sole medium of communication between them.

**The comment trail is the memory.** Every issue accumulates structured comments — INVESTIGATION, DECISION, DEVIATION, COMPLETED, REVIEW, REFACTORING, QA. Any agent or human can reconstruct full context from the comment trail alone. This is what makes session isolation possible: the next agent does not need the previous agent's memory. It needs the issue's history.

**The issue is the contract.** Every agent reads the issue fresh from GitHub. Every agent writes structured comments back to it. The issue is the shared medium between all sessions, all agents, all platforms. If information is not in the comments or the diff, it does not exist for the next agent — and that is correct.

---

## The seven laws

These are not guidelines. They are the invariants of unblock. Violating them is not a configuration option.

1. **GitHub stores, Rust computes.** Zero custom storage. GitHub is the source of truth. unblock is a compute layer.
2. **Every write invalidates and recomputes.** Consistency after mutations. The cache is ephemeral.
3. **The agent is always one command away from productive work.** `prime` → `ready` → `claim` in under two seconds.
4. **Correct GitHub primitive.** Comments for work log. Auto-links for references. Fields for typed data. Body sections for prose. Each data type lives where it belongs.
5. **Session isolation is architectural.** Investigation, implementation, review, and QA run in separate sessions. Enforcement is structural — MCP rejects label transitions without preconditions, the inspector audits after every dispatch, prompts have explicit BLOCK conditions.
6. **Cascade is structural.** Closing an issue recomputes the graph. Newly unblocked issues promote to ready. This is not optional.
7. **One graph, one truth.** The dependency graph is authoritative. If two sources disagree, the graph wins.

---

## How unblock compares

### GitHub Projects V2

GitHub Projects is a view layer — boards, tables, timelines. It organises issues visually but has no dependency graph, no ready queue, no cascade, no claim semantics. An agent looking at a GitHub Project sees a flat list with custom fields. It cannot determine what is blocked, what is ready, or what will unblock when something closes.

unblock reads the same issues and projects but computes the graph that GitHub does not.

### Linear

Linear has cycles, projects, and a beautiful UI. It does not have an MCP interface. It does not expose a dependency graph to agents. It is designed for humans who drag cards on a board, not for agents that need a ready queue and structured tool calls.

### Jira

Jira has everything — including the complexity that comes with everything. Linking issues in Jira does not produce a computable graph. There is no ready set. There is no cascade. An agent interacting with Jira must navigate a REST API designed for human workflows and enterprise configurability.

### beads (bd)

beads is a local-first, git-backed issue tracker optimised for AI agents. It stores issues in a local Dolt database. It has dependencies, ready detection, and agent-friendly JSON output. beads is excellent for single-developer or single-repo workflows where local-first matters.

unblock is different in two ways: it uses GitHub as the source of truth (not a local database), and it exposes its graph via MCP (not a CLI). This makes unblock the choice when the team already lives in GitHub and when multiple agents or humans need to see the same state.

---

## Who unblock is for

unblock is for teams that use GitHub Issues and want their AI agents to manage work — not just write code. Teams where agents claim tasks, implement them, push branches, and cascade unblocks automatically.

unblock is for the developer running Claude Code, Copilot, Cursor, or any MCP-capable client who wants to say `UNBLOCK://ready` and get an answer instead of scrolling through an issue list.

unblock is for the team lead who wants to see — in GitHub, not in a separate tool — which issues are blocked, which are ready, which agent is working on what, and what will unblock when the current sprint closes.

unblock is not for teams that do not use GitHub. unblock is not a generic project management tool. It is a dependency graph for GitHub Issues, exposed via MCP, built for agents.

---

## The three layers

unblock is not one product. It is three layers, each building on the previous.

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
│  UNBLOCK://ready  ://claim  ://close │  cascade, deps — 17 tools
└─────────────────────────────────────┘
```

**Layer 1 — MCP Server.** The foundation. A Rust binary that connects to GitHub, builds the dependency graph in memory, and exposes 17 MCP tools over stdio. Any MCP client connects and operates. The server computes — ready set, cascade, cycle detection, claim — and GitHub stores everything. Zero custom storage. This layer is complete and standalone. An agent with nothing but the MCP server can find work, claim it, implement it, close it, and watch dependents unblock.

**Layer 2 — Plugin.** Specialised agents and skills installed in the editor. The plugin turns the MCP server's raw tools into a structured development pipeline where the developer stays in control and discipline is enforced architecturally — not by instruction alone.

Skills are the unified entry point for both Claude Code and Copilot CLI:

- **Exploration** — `/think` (free exploration: research, PRD, spec, brainstorm — no pipeline enforcement, any agent available)
- **Planning** — `/plan` (two modes: global vision defines phases in the PRD; phase planning produces `plans/NN-plan-{slug}.md` with full detail + GitHub Issues created sequentially)
- **Execution** — `/do` (intent router: implementation, investigation, spec, spike, review, QA — routes to the right agent based on context), `/make` (autonomous execution: same routing, no human-in-the-loop, stricter preconditions)
- **Direct access** — `/use` (dispatch a specific agent by name), `/info` (natural language query over project state), `/trail` (structured narrative history of an issue)
- **Quality gates** — `/ship` (pre-merge readiness check: review passed? QA passed? dependencies closed?)
- **Setup** — `/setup` (bootstrap: GitHub labels, milestone, Projects V2, editor configs, hooks), `/need` (intent-based agent discovery and installation), `/doctor` (diagnostic: MCP health, GitHub state, local environment)

Eight named agents — each a `.md` configuration file, not compiled code — plus dynamic implementation supervisors generated per-project by `/need`:

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

Three enforcement layers make pipeline compliance structurally impossible to bypass:

1. **MCP validation** — the server rejects label transitions when preconditions are not met. No `unblock:review:ok` without a REVIEW comment containing an APPROVE verdict. No `unblock:qa:ok` without a QA comment with a PASS verdict. Enforcement at the infrastructure level.
2. **Inspector (Gadget)** — runs after every agent dispatch. Verifies structured comments exist, are well-formed, and follow the correct sequence. Writes an AUDIT comment only when violations are found. Clean pipelines produce zero noise.
3. **Agent prompt structure** — numbered steps with explicit BLOCK conditions. The agent cannot proceed past a gate without the required artefact. Three independent layers — all three must be bypassed simultaneously to violate the pipeline, which is structurally impossible.

Session isolation is sacred. Investigation, implementation, review, and QA run in separate sessions. The structured comment trail (INVESTIGATION → DECISION → DEVIATION → COMPLETED → REVIEW → REFACTORING → QA) is the sole medium of communication between sessions. Worktrees (`worktrees/issue-{N}-{slug}`) isolate all implementation work.

The plugin works with Claude Code (richest: agents, skills, hooks, `context: fork`), GitHub Copilot CLI (same skills, directive body language for routing), Cursor, and Windsurf.

**Layer 3 — Harness.** The orchestration layer above the plugin. Pre-defined workflows that compose the plugin's agents and skills into coordinated multi-step sequences. A harness takes a high-level intent — "build feature X from scratch", "triage and fix this bug", "plan the next sprint" — and executes the full pipeline automatically: `/think` → `/plan` → `/do` → review → QA. The harness uses agent team patterns — pipeline, fan-out, producer-reviewer, supervisor — to coordinate work across multiple agents. It does not add new capabilities. It composes what the plugin already provides.

Each layer is usable without the one above it. The MCP server is a complete product. The plugin adds agent intelligence and process. The harness adds automation.

---

## The unblock stack

| Layer | Technology | Rationale |
|---|---|---|
| Graph engine | petgraph | Proven Rust graph library. DAG operations, cycle detection, topological sort |
| MCP server | rmcp | Rust MCP SDK. Stdio transport. Tool registration |
| GitHub client | reqwest + GraphQL/REST | Single paginated query for full graph. REST for mutations |
| Error handling | snafu | Structured domain errors with context |
| Logging | tracing | Structured JSON to stderr |
| Async runtime | tokio | Industry standard |
| Distribution | cargo-dist | Cross-platform binaries. Shell + PowerShell installers |

---

*This document is the foundation. Everything else is implementation detail.*
