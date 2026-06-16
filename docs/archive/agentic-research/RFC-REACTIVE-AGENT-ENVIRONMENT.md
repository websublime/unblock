# RFC: Reactive Agent Environment — ://unblock as an Agentic Collaboration Platform

**Status:** DRAFT
**Author:** Miguel Ramos
**Date:** 2026-05-20
**Companion:** [docs/MANIFESTO.md](docs/MANIFESTO.md) (APPROVED), [docs/PRD.md](docs/PRD.md) (APPROVED)
**Prior art:** [MOZAIK-ARCHITECTURE-REFERENCE.md](MOZAIK-ARCHITECTURE-REFERENCE.md) — complete source-level analysis of [Mozaik](https://github.com/jigjoy-ai/mozaik) v3.9.5
**Implementation spec:** [UNBLOCK-AGENTIC-RUST.md](UNBLOCK-AGENTIC-RUST.md) — Rust crate design (`crates/unblock-agentic/`) translating Mozaik + baro patterns
**Affects:** `mcp` service, `workitems` service, `deps` service, Pub/Sub topics, MCP transport layer, `crates/` workspace
**Phase target:** P02+ (after backend MVP; additive, not blocking)

---

## 1. Problem

Today `://unblock` exposes 18 MCP tools over Streamable HTTP. An agent connects, calls a tool, receives a result, disconnects. This is **request/response** — the agent decides when to act, and the system is passive between calls.

This creates three gaps that the current architecture does not address:

1. **No push-based awareness.** When Agent A closes a work item and cascades unblock three dependents, Agent B (working on the same project) has no way to learn this happened until it polls `ready` again. The system computed the cascade, emitted `workitem.changed` internally, but the information never reached the external agent.

2. **No multi-agent coordination.** Two agents working on related items in the same project cannot observe each other's actions. Agent A cannot know that Agent B just posted an INVESTIGATION comment on a sibling item, or that Agent B's function call failed and the item reverted. Each agent operates in isolation, duplicating context and potentially conflicting.

3. **No reactive composition.** Adding behaviours like "auto-assign the next ready item when a dependency resolves", "run a quality gate after every state transition", or "log every agent action to an audit trail" requires either polling loops on the agent side or custom backend logic hardcoded per behaviour. There is no general-purpose mechanism for participants to react to events in the system.

The Manifesto states: _"Agents are first-class users"_ (Principle 5) and _"The graph is the product"_ (Principle 1). First-class users should be notified when the graph changes — not forced to ask.

---

## 2. Proposal

Evolve `://unblock` into a **two-layer reactive system**: a local daemon that orchestrates AI agents on the developer's machine, and a remote backend that stores state and broadcasts events to the team.

**Layer 1 — Local bus (`unblock-agentic` daemon).** A background process on the developer's machine, implemented as the `unblock-agentic` Rust crate (see [UNBLOCK-AGENTIC-RUST.md](UNBLOCK-AGENTIC-RUST.md)). It runs an `AgenticEnvironment` (the bus) per project, spawns Claude Code sessions in parallel based on the dependency graph (DAG), and coordinates them via reactive events. The bus is in-process — all participants (Conductor, Claude Code sessions, observers) run in the same process and communicate through Tokio broadcast channels. The code lives on the developer's filesystem; Claude Code sessions have direct access to it.

**Layer 2 — Remote state (`://unblock` backend).** The Encore Go backend on Encore Cloud. It stores work items, dependencies, comments, and project memory in Postgres. The daemon connects to it via MCP (authenticated per-project with API keys) to read/write work item state. The backend emits events via SSE when state changes (item closed, deps cascaded, item became ready). The daemon receives these events and feeds them into the local bus. The Astro web client also connects to the backend and shows the same events — other developers see what's happening in real time without needing to be on the same machine.

```
Developer's machine                          Remote
┌──────────────────────────────────┐
│  unblock-agentic daemon          │
│                                  │
│  ┌─ Bus (project A) ──────────┐ │         ┌─ ://unblock backend ──────┐
│  │ Conductor (DAG engine)     │ │         │                           │
│  │ Claude Code session 1 ─────┼─┼── MCP ──► workitems service        │
│  │ Claude Code session 2 ─────┼─┼── MCP ──► deps service (cascade)   │
│  │ QualityGate observer       │ │         │ memory service            │
│  │ AuditLogger observer       │ │    SSE ◄── Pub/Sub events          │
│  └────────────────────────────┘ │         │                           │
│                                  │         │         │                │
│  ┌─ Bus (project B) ──────────┐ │         │         ▼                │
│  │ Conductor (DAG engine)     │ │         │  ┌─ Astro Web ────────┐  │
│  │ Claude Code session 3 ─────┼─┼── MCP ──►  │ Kanban board       │  │
│  │ Claude Code session 4      │ │         │  │ Dep graph           │  │
│  └────────────────────────────┘ │         │  │ Agent activity feed │  │
│                                  │         │  └────────────────────┘  │
│  Config: ~/.unblock/config.toml  │         │  (visible to whole team) │
└──────────────────────────────────┘         └──────────────────────────┘
```

The two layers are independent. The daemon can run without the web UI. The web UI can show state without any daemon connected. Multiple developers can run their own daemons against the same project — the backend handles claim contention atomically (`SELECT FOR UPDATE`).

---

## 3. Conceptual Model

### 3.1 The Two Layers In Detail

**The local bus** is an `AgenticEnvironment` instance (Tokio broadcast channel) running inside the daemon process. It coordinates participants that live in the same process: the Conductor (DAG engine), spawned Claude Code sessions, and observers (audit logger, quality gates). Events on the local bus are in-process, zero-latency, and never leave the developer's machine. The bus does not call any LLM — it orchestrates Claude Code sessions that have their own models internally.

**The MCP bridge** connects each local bus to its corresponding project on the ://unblock backend. When a Claude Code session calls an MCP tool (e.g., `claim`, `close`, `post_comment`), the request goes to the backend, which mutates Postgres and emits Pub/Sub events. The daemon maintains an SSE connection per project and receives those events back. When the backend reports "item X is now ready" via SSE, the daemon translates that into a local bus event that the Conductor reacts to — spawning the next Claude Code session.

The flow for a single work item:

```
1. Daemon starts, connects to ://unblock via MCP
2. Daemon calls ready() → backend returns items A, B, C
3. Conductor computes DAG: A and B are parallelisable
4. Conductor emits spawn events on LOCAL BUS
5. StoryFactory spawns claude code session for A (local filesystem)
6. StoryFactory spawns claude code session for B (local filesystem)
7. Session A finishes → calls close(A) via MCP → BACKEND
8. Backend cascades deps → item D becomes ready → emits SSE event
9. Daemon receives SSE → translates to LOCAL BUS event
10. Conductor reacts → spawns session for D
```

Steps 1-6 are local bus. Steps 7-9 cross the MCP bridge. Step 10 is local bus again.

### 3.2 Participants

Every connected entity is a Participant. The system distinguishes participants by capability, not by type:

| Capability         | Description                                      | Who has it                           |
| ------------------ | ------------------------------------------------ | ------------------------------------ |
| `InputCapable`     | Can send messages / comments into the environment | Agents, Humans                       |
| `ActionCapable`    | Can call MCP tools (mutate work items, deps)      | Agents, Automated processes          |
| `InferenceCapable` | Can reason and produce model outputs              | AI Agents only                       |
| `ObserveOnly`      | Receives events but never produces mutations      | Audit loggers, metrics, UI streamers |

A single participant can hold multiple capabilities. A Claude Code agent is `InputCapable + ActionCapable + InferenceCapable`. The Astro web client acting on behalf of a human is `InputCapable + ActionCapable`. An audit logger is `ObserveOnly`.

### 3.3 Events

Every action within the environment produces a typed `EnvironmentEvent`. Events carry the identity of the source participant, the project (environment) scope, and the payload.

**Event taxonomy:**

| Event                      | Triggered when                                          | Payload                                              |
| -------------------------- | ------------------------------------------------------- | ---------------------------------------------------- |
| `participant.joined`       | A participant connects to the project environment       | participant_id, capabilities, agent metadata          |
| `participant.left`         | A participant disconnects                               | participant_id, reason                                |
| `workitem.created`         | A new work item is created                              | work item snapshot                                   |
| `workitem.state_changed`   | A work item transitions state (open→claimed, etc.)      | item_id, old_state, new_state, source_participant     |
| `workitem.claimed`         | A work item is atomically claimed by a participant      | item_id, claimant_participant_id                      |
| `workitem.comment_added`   | A structured comment is posted (INVESTIGATION, REVIEW…) | item_id, comment_type, source_participant              |
| `workitem.closed`          | A work item reaches terminal state                      | item_id, resolution                                  |
| `deps.cascade`             | Dependency resolution cascaded — new items unblocked    | newly_ready_item_ids[], trigger_item_id               |
| `deps.cycle_detected`      | A cycle was detected in the dependency graph            | cycle_path[]                                         |
| `provider.webhook`         | An external provider event was ingested                 | provider, event_type, normalised payload              |
| `memory.updated`           | A project/org memory entry was created or changed       | scope, key, source_participant                        |
| `tool.called`              | Any MCP tool was invoked                                | tool_name, input_summary, source_participant           |
| `tool.result`              | An MCP tool returned a result                           | tool_name, success/failure, output_summary             |
| `pipeline.gate_passed`     | A pipeline quality gate was satisfied                   | item_id, gate (investigate/implement/review/quality)  |
| `pipeline.gate_blocked`    | A pipeline quality gate blocked a transition            | item_id, gate, reason                                |

### 3.4 Self vs. External — The Reactive Split

Following Mozaik's pattern, translated in `unblock-agentic` as two trait methods (`on_self_event` and `on_external_event`), each participant distinguishes between its own actions and others' actions:

- **Self events** (`on_self_event`): "I called `claim` and it succeeded" — allows the participant to update its own internal state (add to context, trigger next step).
- **External events** (`on_external_event`): "Another agent claimed an item I was looking at" — allows the participant to react to others' actions (update awareness, start complementary work, log).

Each participant's event loop compares `event.source` with its own `ParticipantId` and routes to the appropriate handler. The `unblock-agentic` crate handles this routing in `spawn_participant()` — see UNBLOCK-AGENTIC-RUST.md §4.2 for the implementation.

This split enables composition by reaction:

- **Agent A** overrides `onExternalWorkitemClosed` → sees that a blocker was resolved → calls `ready` → claims the next item. Zero orchestration.
- **Quality Gate Observer** overrides `onExternalStateChanged` → sees a transition to `review` → checks that an INVESTIGATION comment exists → either allows or blocks. No coupling to Agent A.
- **Audit Logger** overrides all `onExternal*` handlers → writes every action to a JSONL audit trail. Never mutates anything.
- **Cascade Notifier** overrides `onDepsCascade` → sends a summary to the Astro web client so the human sees "3 items just became ready".

None of these participants know about each other. They compose through shared events.

---

## 4. Mapping to Existing Architecture

The two-layer model requires changes in two places: a new Rust binary (the daemon) and a small addition to the existing backend (SSE event broadcast).

### 4.1 Layer 1 — The Daemon (`unblock-agentic` binary)

A new Rust binary in the `crates/` workspace. It is the local orchestrator that runs on the developer's machine.

**Configuration:** `~/.unblock/config.toml`

```toml
[daemon]
log_level = "info"
socket = "~/.unblock/daemon.sock"

[[projects]]
name = "unblock-v1"
org = "websublime"
api_key = "ub_key_abc..."
endpoint = "https://api.unblock.websublime.com"
cwd = "/home/miguel/code/unblock"
max_parallel = 3
auto_run = true

[[projects]]
name = "client-project"
org = "acme-corp"
api_key = "ub_key_xyz..."
endpoint = "https://unblock.acme-corp.com"
cwd = "/home/miguel/work/acme-frontend"
max_parallel = 2
auto_run = true
```

Each project entry is fully independent: its own org, API key, endpoint, and working directory. The daemon creates one `AgenticEnvironment` (local bus) and one MCP/SSE connection per project.

**CLI interface:**

```bash
unblock-agentic daemon start          # launch background process
unblock-agentic daemon stop           # stop everything
unblock-agentic daemon status         # show all projects

unblock-agentic status                # summary across all projects
unblock-agentic status unblock-v1     # detail for one project
unblock-agentic run unblock-v1        # manual trigger (auto_run=false projects)
unblock-agentic pause unblock-v1      # pause without disconnecting
unblock-agentic logs unblock-v1       # tail logs
unblock-agentic logs unblock-v1 -f    # follow mode

unblock-agentic project add           # interactive wizard
unblock-agentic project remove line-ui
```

The CLI communicates with the running daemon via unix socket (`~/.unblock/daemon.sock`).

### 4.2 Layer 2 — Backend SSE Event Broadcast

The backend needs one addition: the `mcp` service subscribes to the three existing Encore Pub/Sub topics and forwards events to connected SSE clients.

| Existing Pub/Sub topic | SSE events broadcast to daemon                                        |
| ---------------------- | --------------------------------------------------------------------- |
| `workitem.changed`     | `workitem.created`, `workitem.state_changed`, `workitem.claimed`, `workitem.closed`, `workitem.comment_added` |
| `deps.recomputed`      | `deps.cascade`, `deps.cycle_detected`                                  |
| `provider.events`      | `provider.webhook`                                                     |

Events are delivered as MCP server-to-client notifications (spec 2025-06-18):

```json
{
  "jsonrpc": "2.0",
  "method": "notifications/unblock/event",
  "params": {
    "project_id": "proj_01JXY...",
    "event": "deps.cascade",
    "timestamp": "2026-05-20T14:30:00Z",
    "payload": {
      "trigger_item_id": "wi_01JXY...",
      "newly_ready": ["wi_01JXZ...", "wi_01JXW..."]
    }
  }
}
```

The daemon receives this notification, translates it into a `CustomEvent` on the local bus, and the Conductor reacts by spawning Claude Code sessions for the newly-ready items.

### 4.3 What Stays on the Backend vs. What Moves to the Daemon

| Concern | Where it lives | Why |
|---|---|---|
| Work item state (CRUD, transitions) | Backend (Postgres) | Source of truth, shared across all developers |
| Dependency graph computation (cascade) | Backend (deps service) | Must be atomic and consistent across concurrent claims |
| Claim contention (`SELECT FOR UPDATE`) | Backend | Two developers' daemons might race for the same item |
| DAG level computation (what's ready) | Daemon (local) | The Conductor uses this to decide what to spawn next |
| Agent spawning (Claude Code sessions) | Daemon (local) | Needs filesystem access |
| Quality gates (pipeline validation) | Both | Backend enforces state machine; daemon can add local pre-checks |
| Audit logging | Both | Backend persists to Postgres; daemon writes local JSONL |
| Real-time visualisation | Backend → Astro web | The web client reads from the backend, not from any daemon |

### 4.4 How the Astro Web Client Fits

The Astro web client connects to the backend, not to any daemon. It sees the same Pub/Sub events that daemons see, but renders them visually. When Developer A's daemon closes a work item, the backend emits a Pub/Sub event. Both Developer B's daemon and the Astro web client receive it — Developer B's daemon spawns the next agent, and the web UI updates the kanban board.

No daemon needs to be running for the web UI to work. The web UI shows the state of the project as stored in Postgres, not the state of any local process.

---

## 5. Impact on mister-anderson and the Plugin Model

### 5.1 Before: Plugin as Orchestrator

Today the mister-anderson plugin operates as a centralised pipeline inside Claude Code. The personas (Sherlock, Linus, Quinn, etc.) are prompts orchestrated sequentially by the plugin. The plugin decides: "Sherlock investigates → Linus specifies → Quinn implements". Context flows within a single Claude Code session.

### 5.2 After: Personas as Independent Participants

With the reactive environment, each persona can operate as an independent agent connected to `://unblock`:

| Persona    | Role                  | Reactive behaviour                                                     |
| ---------- | --------------------- | ---------------------------------------------------------------------- |
| Sherlock   | Investigator          | Subscribes to `workitem.claimed` where stage=investigate. Reads item context + memory, posts INVESTIGATION comment, transitions state. |
| Linus      | Specifier             | Subscribes to `pipeline.gate_passed` where gate=investigate. Pulls item, writes spec, transitions to implement. |
| Quinn      | Implementer           | Subscribes to `pipeline.gate_passed` where gate=specify. Claims item, writes code, transitions to review. |
| Martin     | Reviewer              | Subscribes to `workitem.state_changed` where new_state=review. Reviews, posts REVIEW comment, passes/fails. |
| Fernando   | QA                    | Subscribes to `pipeline.gate_passed` where gate=review. Runs quality checks, closes or rejects. |

The pipeline emerges from the event flow: Sherlock's output triggers Linus, Linus's output triggers Quinn, Quinn's output triggers Martin, Martin's output triggers Fernando. No central scheduler. No single-process constraint. Each persona can be a separate Claude Code session, a different model, or even a non-LLM automated process.

### 5.3 What mister-anderson Becomes

The plugin doesn't disappear — it evolves:

- **Local mode (unchanged):** For single-developer use, the plugin continues to orchestrate personas within a single Claude Code session. This is the existing behaviour and remains the default for quick iterations.
- **Distributed mode (new):** For larger projects or multi-agent workflows, the plugin acts as a **launcher** — it spawns or connects persona-agents to the `://unblock` environment and lets the reactive event flow handle coordination. The plugin's role shifts from "scheduler" to "deployer + monitor".
- **Beads integration obsoleted:** Since `://unblock` is the tracker, Beads' role (JSONL export to git, task state) is absorbed natively. Every work item transition is already persisted in Postgres with full audit trail.

### 5.4 Migration Path

This is not a breaking change. The reactive environment is additive:

1. **Phase 1 (P02):** Implement the `unblock-agentic` crate (`crates/unblock-agentic/`) per UNBLOCK-AGENTIC-RUST.md — bus, participants, DAG. Unit-tested in isolation, no backend integration yet.
2. **Phase 2 (P02):** Wire the `mcp` service to use `unblock-agentic` internally. Add event broadcast over SSE using the `CustomEvent` variant for ://unblock domain events. Agents that don't subscribe simply ignore the notifications — existing tool-call workflows work unchanged.
3. **Phase 3 (P04+):** Build the first reactive observers (audit logger, quality gate) as `Participant` implementations. Validate the event schema and delivery guarantees.
4. **Phase 4 (post-v1.0):** Implement persona-agents (Sherlock, Linus, Quinn) as independent `Participant` instances connected via MCP. Test multi-agent collaboration on a real project.
5. **Phase 5 (v1.1+):** Expose participant presence and event stream in the Astro web client. Humans see agents working in real time.

---

## 6. Design Constraints

1. **No new infrastructure.** The reactive layer uses existing Encore Pub/Sub and SSE. No message broker, no Redis, no WebSocket server.
2. **Backward compatible.** Agents that never subscribe to events continue to work exactly as today. The event stream is opt-in.
3. **Postgres remains canonical.** Events are derived from state changes in Postgres. The event stream is a projection, not a source of truth. If an agent misses an event (disconnect, network), it can reconstruct state from the database via existing MCP tools.
4. **Backpressure by design.** If a subscriber falls behind, the SSE connection buffers up to N events and then drops the oldest. The subscriber can resync via `ready` or `list_work_items`. This prevents a slow agent from degrading the system.
5. **Security scoping.** A participant only receives events for the project(s) its API key is authorised for. Cross-project event leakage is a security violation.
6. **Event ordering.** Events within a single project are delivered in causal order (Postgres transaction commit order via Pub/Sub). Cross-project ordering is not guaranteed and not needed.

---

## 7. What This Enables (Scenarios)

### 7.1 Parallel Agent Pipeline

Five Claude Code agents connect to the same project. Each subscribes to different pipeline gates. A human creates 10 work items with a dependency graph. The first agent claims the first leaf item, investigates, and transitions it. The cascade triggers the next agent automatically. Within minutes, multiple items are being worked in parallel by different agents, each reacting to the events of the previous stage. No human orchestration beyond the initial work item creation.

### 7.2 Self-Healing Quality Gate

An observer participant monitors `workitem.state_changed`. When it sees a transition to `review` without a preceding `INVESTIGATION` comment of type `investigation`, it automatically reverts the state and emits a `pipeline.gate_blocked` event with a reason. The implementing agent receives the block notification and can self-correct.

### 7.3 Live Project Dashboard

The Astro web client connects as an `ObserveOnly` participant. It receives every event in real time and updates the kanban board, dependency graph, and agent activity feed without polling. The human sees agents working — items moving across columns, comments appearing, dependencies resolving — as it happens.

### 7.4 Cross-Project Orchestration (v2.0+)

A meta-orchestrator agent connects to multiple project environments simultaneously. It observes progress across projects and can trigger cross-project actions: "when all v1.0 blockers in project A are resolved, create the release work item in project B".

---

## 8. Relationship to Existing Documents

| Document                       | Impact                                                                |
| ------------------------------ | --------------------------------------------------------------------- |
| MANIFESTO.md                   | No changes. The reactive environment strengthens Principles 1 (graph is the product), 5 (agents are first-class), and 7 (structured state, not free-form). |
| PRD.md                         | Additive. New user stories for reactive subscription (US-NEW-1: "subscribe to project events"), multi-agent coordination (US-NEW-2: "observe other agents' actions"), and participant lifecycle (US-NEW-3: "know who is working on what"). These extend Section 4 without modifying existing stories. |
| SPEC.md                        | Extends the MCP service specification with event broadcast, participant registry, and new Pub/Sub topics. Does not change existing service interfaces. |
| CLAUDE.md                      | Add reactive environment awareness to the operator manual: how persona-agents connect, how events flow, how distributed mode differs from local mode. |
| MOZAIK-ARCHITECTURE-REFERENCE.md | Read-only reference. Source-level analysis of the prior art that inspired this RFC. No changes expected. |
| UNBLOCK-AGENTIC-RUST.md        | Implementation specification for `crates/unblock-agentic/`. This RFC defines the *why* and *what*; that document defines the *how* at the Rust API level. Phase plans should derive implementation tasks from it. |

---

## 9. Open Questions

1. **Event filtering granularity.** Should participants declare subscription filters at connect time (e.g., "only events for items I claimed"), or receive everything and filter client-side? Server-side filtering reduces bandwidth but adds complexity to the registry.

2. **Event persistence.** Should the event stream be persisted (append-only table) for replay, or is it ephemeral? Persistence enables late joiners to catch up but adds storage cost. The current design leans ephemeral with database state as the resync mechanism.

3. **Rate limiting.** Should there be per-participant event rate limits to prevent a runaway observer from being overwhelmed? Or is backpressure (buffer + drop) sufficient?

4. **Participant identity.** Should multiple Claude Code sessions sharing the same API key be treated as one participant or many? The current design suggests one participant per SSE connection, but this has implications for claim contention.

5. **MCP spec alignment.** The MCP specification is evolving. Should `://unblock` events use the standard `notifications/*` namespace or a custom `unblock/*` namespace? The current design uses `notifications/unblock/event` to stay within spec while being distinguishable.

---

## 10. Decision Requested

This RFC proposes adding a reactive event layer to `://unblock` that transforms the platform from a passive tool server into an agentic collaboration environment. The change is additive, backward-compatible, and builds on existing infrastructure (Pub/Sub, SSE, MCP notifications).

The implementation is specified in two companion documents: [MOZAIK-ARCHITECTURE-REFERENCE.md](MOZAIK-ARCHITECTURE-REFERENCE.md) (prior art analysis) and [UNBLOCK-AGENTIC-RUST.md](UNBLOCK-AGENTIC-RUST.md) (Rust crate design for `crates/unblock-agentic/`).

The request is to approve this RFC as a design input for the SPEC.md system architecture (Stage 1, pending) and to schedule the `unblock-agentic` crate implementation as part of Phase 2 (P02) of the roadmap.

---

## Appendix A: Reference Documents

### A.1 MOZAIK-ARCHITECTURE-REFERENCE.md

Complete source-level analysis of Mozaik v3.9.5 (782 LOC, 43 files). Covers every interface, class, handler, and data flow in the original TypeScript framework. 7 Mermaid diagrams. Documents what Mozaik does and — critically — what it does NOT do (no persistence, no auth, no backpressure, no custom events, no error isolation). This document is the "what exists" reference.

### A.2 UNBLOCK-AGENTIC-RUST.md

Complete crate design for `crates/unblock-agentic/` — the Rust translation of Mozaik + baro's extensions. Defines every trait, struct, and enum. Three modules: Bus (AgenticEnvironment with Tokio broadcast/mpsc, EnvironmentEvent enum, CustomEvent), Participants (Participant trait with on_self_event/on_external_event, 3 concrete types), and DAG (Kahn's topological sort for dependency-ordered execution). 16 public types. This document is the "how to build it" specification.

### A.3 Baro (jigjoy-ai/baro)

Claude agent orchestrator built on Mozaik with 14 specialised participants (Conductor, StoryFactory, StoryAgent, Librarian, Sentry, Critic, Surgeon, Operator, Auditor, Cartographer, Finalizer, plus OpenAI variants) working concurrently on the same bus. Key patterns extracted into `unblock-agentic`:

- **BusEvent extension** — Mozaik's built-in event types weren't sufficient; baro patched in a custom `deliverBusEvent` channel. In `unblock-agentic` this is the `CustomEvent` variant, built-in from day one.
- **DAG-driven execution** — baro's Conductor uses Kahn's topological sort to compute parallelisable levels from a dependency graph. In `unblock-agentic` this is the `build_dag()` function.
- **Factory pattern** — baro's Conductor never spawns agents directly; it emits `StorySpawnRequestItem` events and a `StoryFactory` participant reacts by creating agents. This decoupling pattern applies directly to ://unblock's work item assignment.
- **Observer composition** — baro demonstrates that logging (Auditor), conflict detection (Sentry), knowledge sharing (Librarian), quality evaluation (Critic), and adaptive replanning (Surgeon) can all be independent observer participants on the same bus, composed without touching the core orchestration logic.
