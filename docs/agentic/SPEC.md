# SPEC: `unblock-agentic` — DAG-Driven Control-Plane Daemon

**Status:** DRAFT
**Author:** Ada (architect)
**Date:** 2026-06-18
**Source PRD:** [docs/agentic/PRD.md](./PRD.md) (APPROVED, 2026-06-18 — RD-1…RD-6 + SQ-1…SQ-5 + transport + hook-sink locked)
**Design input:** `/tmp/unblock-agentic-design-v2.md` (design LOCKED 2026-06-17; the PRD supersedes it wherever they differ)
**Companion:** [docs/MANIFESTO.md](../MANIFESTO.md) (APPROVED) — the 8 governing Laws
**Sibling architecture:** [docs/SPEC.md](../SPEC.md) (`://unblock` Stage-1 architecture) — the substrate this daemon sits on
**Renderer contract:** `unblock-render` packaged-plugin output (commit `69dad9a`)

> This is the implementation contract for `crates/unblock-agentic`. It assumes
> the PRD's locked decisions and does **not** re-open them; its job is to
> specify *how* to implement them — module layout, ledger schema, exact
> commands, the reconcile algorithm, the unix-socket transport protocol, the
> key-seed, the gate sequences, and the test obligations. Backend claims are
> grounded in `apps/api/` with `file:line`. Where the daemon needs a backend
> capability that does not exist, it is flagged in §16 as a required backend
> change out of this SPEC's scope.
>
> **Altitude.** SPEC altitude is expected: module signatures, schemas, exact
> CLI invocations, and algorithms are in scope. Per-phase task decomposition
> (`/tasks`, Fernando) is downstream and out of scope here.

---

## 1. Overview

`unblock-agentic` is a **reconciler / control-plane daemon** (Kubernetes-controller
style). It parallelises the mister-anderson pipeline over the `://unblock`
dependency DAG, keeps the personas + gates + quality bar, and adds the
scheduling, single-writer discipline, completion-verification, and human-gate
machinery that the `claude` CLI alone does not provide.

It is a **single binary** in a new crate `crates/unblock-agentic`, distributed
through the same cargo-dist pipeline as `unblock-code` / `unblock-render`. It
targets the `claude` CLI exclusively (NFR-8). It runs **locally, in the single
operator's name**, serving **one ORG and all of that org's projects** (SQ-1).

### 1.1 What is in this SPEC

- Crate + module layout (§2).
- The `reconcile()` control loop — phases, triggers, idempotency, the work-item
  lifecycle driver (§3).
- The orchestration ledger schema (SQLite, project-qualified, rebuildable from
  truth) (§4).
- Dispatch / session birth — the exact `claude --bg` command, session-id
  capture, worktree provisioning, per-session `.mcp.json` (§5).
- The unix-socket transport — the shim wire protocol, team routing by
  connection, key injection, deny-reshape (§6).
- Keys & the encore-internal seed — `IssueAPIKey`, `CallerUserID` pinning,
  keychain storage (§7).
- Single-writer Model-2 — the desired-DAG apply algorithm (§8).
- Gates, needs-input, findings topology (§9).
- Concurrency budget — org-wide ceiling + per-project quota + admission (§10).
- Degraded mode (§11).
- The control-MCP — tools, OS notifications, optional HTML dashboard (§12).
- Renderer relationship + Law-8 survival + the stub plugin (§13).
- Phasing P0–P3 (§14).
- Manifesto-Law mapping (§15).
- Required backend changes / missing capabilities (§16).
- Resolution of the 8 still-open §13 PRD SPEC-TODOs (§17).
- Test obligations + acceptance criteria (§18).

### 1.2 What is NOT in this SPEC

- Per-phase task beads (`/tasks`).
- The `unblock-render` internals (its own spec/impl, `://unblock` `P04`).
- Backend RPC implementation (grounded here as fixed inputs; changes flagged
  in §16).
- Multi-operator / multi-org / governance (post-v1, §15 of the PRD).

---

## 2. Crate & module layout

A single binary crate, edition 2024, sharing the workspace conventions of
`crates/` (`#![deny(unsafe_code)]`, `snafu` errors, `tracing` JSON Lines on
STDERR, `///` docs on all `pub`).

```
crates/unblock-agentic/
├── Cargo.toml                         # bin "unblock-agentic"
├── data/
│   └── stub-plugin/                   # the P0 conformant stub plugin (§13.3)
│       ├── .claude-plugin/plugin.json
│       ├── agents/                    # coordinator + every referenced teammate persona
│       └── hooks/
└── src/
    ├── main.rs                        # clap dispatch: serve | proxy | ctl | hook | seed
    ├── cli.rs                         # subcommand definitions + arg parsing
    ├── config.rs                      # daemon config (org_id, project set, ceilings, paths)
    ├── daemon/
    │   ├── mod.rs                     # daemon lifecycle: socket listen + tick loop + sink
    │   ├── reconcile.rs               # reconcile(): observe → reconcile-ledger → diff → act
    │   ├── observe.rs                 # `claude agents --json` + ://unblock reads → ObservedState
    │   ├── plan.rs                    # diff → Vec<Action> (idempotent, level-triggered)
    │   ├── act.rs                     # apply Action (dispatch | reshape | gate | promote)
    │   ├── lifecycle.rs               # the per-(project,wi) state driver (§3.4)
    │   └── degraded.rs               # pause + fail-fast + backoff state machine (§11)
    ├── ledger/
    │   ├── mod.rs                     # open/migrate the SQLite ledger
    │   ├── schema.rs                  # DDL constants (§4)
    │   └── repo.rs                    # typed CRUD: assignments, sessions, rework, budget
    ├── dispatch/
    │   ├── mod.rs                     # build + spawn the `claude --bg` command (§5)
    │   ├── worktree.rs                # git worktree create/cleanup, `worktree-<project>-<wi>`
    │   ├── brief.rs                   # brief composition (§9.4, points-not-inlines)
    │   ├── mcpconfig.rs               # render the per-session `.mcp.json`
    │   └── sessionid.rs               # capture the --bg-generated session id (§5.3)
    ├── unblock/
    │   ├── mod.rs                     # remote-MCP client over Streamable HTTP
    │   ├── tools.rs                   # typed wrappers for the 23-tool surface used
    │   ├── reshape.rs                 # the daemon-only reshape set (§8) — reshape key
    │   └── append.rs                 # the lead append surface (used only by the proxy path)
    ├── transport/
    │   ├── mod.rs                     # unix-socket server (operator-only perms) (§6)
    │   ├── frame.rs                   # the shim ↔ daemon length-prefixed JSON-RPC frame
    │   ├── route.rs                   # connection → {team-proxy | control | hook} routing
    │   └── deny.rs                    # reshape-deny enforcement on team routes
    ├── proxy/
    │   └── mod.rs                     # `unblock-agentic proxy --team <X> --session <id>` shim
    ├── control/
    │   ├── mod.rs                     # `unblock-agentic ctl` shim (control-MCP)
    │   ├── tools.rs                   # status/pending/approve/reject/waive/answer
    │   └── notify.rs                  # OS desktop notifications
    ├── hooksink/
    │   ├── mod.rs                     # `unblock-agentic hook` shim → sink
    │   └── events.rs                  # Claude Code hook event payloads + dedup keys
    ├── keys/
    │   ├── mod.rs                     # keychain read/write (never env) (§7)
    │   └── seed.rs                    # CLI front for the encore-internal seed (§7.2)
    └── notify/
        └── dashboard.rs               # OPTIONAL loopback HTML dashboard (post-v1, §12.3)
```

**Subcommands (one binary, five roles).** The daemon and all three shims are
the same binary, dispatched by argv0 subcommand:

| Subcommand | Role | Who runs it |
|---|---|---|
| `unblock-agentic serve` | the daemon (tick loop + socket server + sink) | the operator, once |
| `unblock-agentic proxy --team <X> --session <id>` | per-team data-plane shim | spawned by an agent session's `.mcp.json` |
| `unblock-agentic ctl` | operator control-MCP shim | spawned by the operator's own Claude `.mcp.json` |
| `unblock-agentic hook --event <e> --name <wi>` | hook-sink shim | spawned by Claude Code hooks |
| `unblock-agentic seed …` | the encore-internal key seed front (§7.2) | the operator, once per env |

All three shims (`proxy`, `ctl`, `hook`) connect to the **one operator-only
unix socket** the `serve` daemon listens on (§6). The shim is **thin**: it
speaks stdio JSON-RPC to its Claude-side client and forwards framed messages to
the daemon; it holds no keys and no policy.

---

## 3. The `reconcile()` control loop

### 3.1 Triggers (FR-1)

`reconcile()` runs on two triggers, both funneling into a single serialized
executor (one reconcile pass at a time; a trigger arriving mid-pass sets a
`dirty` flag that schedules exactly one follow-up pass — coalesced, never
queued N-deep):

1. **Event-driven** — a hook frame arrives at the sink (§6.4). The sink does
   NOT mutate truth; it records the hint in the ledger's `hook_hints` table and
   signals the executor.
2. **Periodic tick** — a `tick_interval` timer (default 15 s, configurable).
   This is the self-heal path; it makes the daemon level-triggered (a missed
   hook is recovered by the next tick).

### 3.2 Phases (FR-2 — idempotent, level-triggered)

One pass is four phases, in order. The pass is **idempotent**: running it twice
with no intervening change produces no additional side effects (every `act`
operation is guarded by a ledger-recorded precondition or a backend idempotent
op).

```
observe → reconcile-ledger → diff → act
```

- **observe** (`observe.rs`) — read OBSERVED state:
  - `claude agents --json` → the live session set `{pid, cwd, kind, startedAt,
    sessionId, name, state}` (NFR-8). `kind ∈ {interactive, background}` is NOT
    team-vs-operator, so sessions are namespaced by `--name` (the work-item id)
    and `--cwd` (the worktree path).
  - For each project in the org's project set: `://unblock` `ready`,
    `get_state`, and `show`/`list` for the items the ledger knows about. Truth
    is the union of these two sources. Hooks are hints only (FR-3).
- **reconcile-ledger** (`reconcile.rs`) — correlate the buffered `hook_hints`
  to truth and to ledger assignments by `(--name, --cwd)`; capture any
  not-yet-recorded `--bg`-generated session id (§5.3); drop stale assignments
  whose session no longer appears in `agents --json` AND whose item is not
  complete (these become re-dispatch candidates per the restart invariant,
  FR-5).
- **diff** (`plan.rs`) — for every `(project, wi)` the daemon is responsible
  for, compute the desired next action from `(observed-state, ledger)` via the
  lifecycle driver (§3.4). Output: `Vec<Action>`. **Pure** — no side effects.
- **act** (`act.rs`) — apply each `Action`. Actions are: `Dispatch`,
  `ApplyReshape`, `Promote`, `GateApprove`/`GateReject`/`GateWaive`,
  `PostNeedsInputAnswer`, `Notify`. Each is independently idempotent.

### 3.3 Truth precedence (FR-3, Law 3)

On any conflict, `claude agents --json` + `://unblock` win over hook hints.
Hook hints are advisory and may lag or be missed; they only ever *accelerate* a
reconcile (event trigger) or *enrich* correlation. The daemon NEVER advances a
work item on a hint alone — verification is always against `://unblock`
artifacts + the comment trail (FR-13, NFR-2).

### 3.4 The per-`(project,wi)` lifecycle driver (`lifecycle.rs`, FR-4)

The reconciler works at **work-item granularity**. For each work item, given
its observed state, it computes the next action. The driver is a pure function:

```
fn next_action(obs: &ItemObservation, led: &LedgerView, budget: &BudgetView)
    -> Option<Action>
```

States and the transitions the driver drives (grounded in the backend `Status`
enum `Backlog | Ready | InProgress | Blocked | Done` and the four state columns
— `apps/api/workitems/workitems.go:1785` reads `impl_state, review_state,
qa_state, pipeline_state, claimed_by_id`):

| Observed | Ledger | Action |
|---|---|---|
| `Backlog` AND `is_ready=true` | (none) | `Promote` (daemon-owned, §8) |
| `Ready`, unclaimed, no assignment | budget admits (§10) | `Dispatch` (§5) |
| `Ready`/`InProgress`, assignment exists, session live | — | none (in-flight) |
| `InProgress`, claimed, `kind=completed` on trail, gate Area | — | project **GatePending** (§9.1); `Notify` operator |
| `pipeline_state=needs_human` + `kind=needs-human` comment | — | project **NeedsInput** (§9.3); `Notify` operator |
| assignment exists, session GONE, item incomplete | — | `Dispatch` (re-dispatch, restart invariant FR-5) |
| session `state:blocked` | — | treat as failure (FR-14): `Dispatch` re-dispatch OR project NeedsInput |
| gate `review_state=approved` requested by operator | — | `GateApprove` = `set_state(review_state=approved) → close` (§9.2) |
| dependent left `Backlog`+`is_ready` after a blocker closed | — | `Promote` (§9.2 cascade tail) |

The driver NEVER trusts in-memory counters: `inflight` is recomputed from
`claude agents --json` each pass (FR-34); rework counts are derived from the
comment trail each pass (FR-27, §9.5). The ledger holds only durable
orchestration facts, never derived truth (§4).

### 3.5 Restart invariant (FR-5, NFR-1, SM-3)

A daemon restart is just the next reconcile pass. On startup the daemon:
1. opens + migrates the ledger;
2. runs `observe` (reads `agents --json` + `://unblock`);
3. in `reconcile-ledger`, re-correlates persisted assignments to live sessions;
4. for any assignment whose session is gone and whose item is incomplete,
   queues a re-dispatch (teammates do NOT resume — documented Claude Code
   limitation, R-1).

Bounded wasted work (a re-dispatched team redoing a partly-done area-task) is
the accepted cost. **SM-3 (HARD, gates P0):** after `kill -9`, the next tick
recovers to correct in-flight state within one reconcile pass — no leaked
slots, no double-dispatch, no lost gate.

---

## 4. The orchestration ledger

**Storage.** A local **SQLite** database (WAL mode) at
`$XDG_DATA_HOME/unblock-agentic/<org_id>/ledger.db` (fallback
`~/.local/share/...`; macOS `~/Library/Application Support/...`). SQLite is
chosen for parity with `unblock-code`'s storage stack (`rusqlite`/`sqlx`
already in the workspace) and because the ledger is single-writer (the daemon)
and small. **The ledger is orchestration-only state — never domain truth**
(NFR-1, NFR-10, Law 3). `bd`/beads/Dolt is NEVER in the runtime (NFR-10).

**Project-qualification (SQ-1, FR-12).** Every row is keyed by `(project_id,
wi_id)`, never `wi_id` alone, so the same work-item number across two of the
org's projects never collides.

**Rebuildable from truth.** Nothing in the ledger is authoritative. On a fresh
ledger (or a corrupt one), the daemon reconstructs all orchestration facts from
`claude agents --json` + `://unblock` + the worktree filesystem. The ledger is
a cache/journal that makes correlation cheap and prevents double-dispatch
within a pass.

```sql
-- 4.1 assignments — one row per dispatched area-task (FR-12)
CREATE TABLE assignments (
    org_id        TEXT NOT NULL,
    project_id    TEXT NOT NULL,
    wi_id         TEXT NOT NULL,          -- ://unblock work-item id (ULID)
    team          TEXT NOT NULL,          -- one of the 8 roster teams (§7.2 PRD)
    area_kind     TEXT NOT NULL,          -- 'gate' | 'pool'
    worktree_path TEXT NOT NULL,          -- worktree-<project>-<wi>
    session_id    TEXT,                   -- captured --bg session id (NULL until captured, §5.3)
    session_name  TEXT NOT NULL,          -- the --name correlation key (= wi_id)
    dispatched_at TEXT NOT NULL,          -- RFC3339
    status        TEXT NOT NULL,          -- 'dispatched'|'observed'|'completed'|'failed'|'redispatch'
    brief_digest  TEXT NOT NULL,          -- sha256 of the brief sent (audit; brief itself not stored)
    PRIMARY KEY (org_id, project_id, wi_id)
);

-- 4.2 captured sessions — history of --bg session ids per area-task (R-8)
CREATE TABLE sessions (
    org_id     TEXT NOT NULL,
    project_id TEXT NOT NULL,
    wi_id      TEXT NOT NULL,
    session_id TEXT NOT NULL,
    captured_at TEXT NOT NULL,
    PRIMARY KEY (org_id, project_id, wi_id, session_id)
);

-- 4.3 rework_counters — CACHE of the trail-derived count (FR-27, §9.5).
--     Authoritative source is the comment trail; this row is recomputed each
--     pass and exists only so a notification fires exactly once per increment.
CREATE TABLE rework_counters (
    org_id        TEXT NOT NULL,
    project_id    TEXT NOT NULL,
    wi_id         TEXT NOT NULL,
    count         INTEGER NOT NULL DEFAULT 0,  -- = trail-derived count last pass
    escalated     INTEGER NOT NULL DEFAULT 0,  -- 1 once 3x escalation fired (dedup)
    PRIMARY KEY (org_id, project_id, wi_id)
);

-- 4.4 budget_reservations — transient dispatch admissions within a pass (§10).
--     Cleared and rebuilt each pass from `agents --json`; persisted only to
--     survive a mid-pass crash without leaking a slot.
CREATE TABLE budget_reservations (
    org_id     TEXT NOT NULL,
    project_id TEXT NOT NULL,
    wi_id      TEXT NOT NULL,
    reserved_at TEXT NOT NULL,
    PRIMARY KEY (org_id, project_id, wi_id)
);

-- 4.5 hook_hints — buffered, deduped hook events (FR-16). Hints, not truth.
CREATE TABLE hook_hints (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    dedup_key   TEXT NOT NULL UNIQUE,     -- sha256(event_type|session_name|payload_id)
    event_type  TEXT NOT NULL,           -- SessionStart|Stop|SubagentStop|TeammateIdle|TaskCompleted
    session_name TEXT NOT NULL,          -- the --name (= wi_id)
    received_at TEXT NOT NULL,
    consumed    INTEGER NOT NULL DEFAULT 0
);

-- 4.6 gate_verdicts — operator verdicts captured by the control-MCP (§9, §12),
--     pending application by the next reconcile pass (idempotency boundary).
CREATE TABLE gate_verdicts (
    org_id     TEXT NOT NULL,
    project_id TEXT NOT NULL,
    wi_id      TEXT NOT NULL,
    verdict    TEXT NOT NULL,            -- 'approve'|'reject'|'waive'|'answer'
    payload    TEXT,                     -- e.g. the needs-input answer body
    created_at TEXT NOT NULL,
    applied_at TEXT,                     -- NULL until the reconciler applies it
    PRIMARY KEY (org_id, project_id, wi_id, created_at)
);
```

**Migrations.** The ledger ships forward-only SQL migrations under
`src/ledger/` embedded via `include_str!`, applied on open by a `user_version`
pragma check. No editing an applied migration in place (the same
forward-migration discipline the backend follows, PRD §10.4).

---

## 5. Dispatch / session birth (FR-9…FR-15)

### 5.1 The dispatch command

A dispatch is one `claude --bg` lead per area-task. The canonical command
(verified `claude` v2.1.178, PRD §10 / design v2 §10):

```
CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1 claude --bg \
  --agent <coordinator-persona> \
  --plugin-dir <rendered-plugin-or-stub> \
  --mcp-config <session .mcp.json> \
  --strict-mcp-config \
  --permission-mode default \
  --allowedTools "<append+role tools; deny Edit/Write on .mcp.json|.claude/settings>" \
  --name <wi_id> \
  --cwd <worktree-<project>-<wi>> \
  -p "<brief — points to spec/plan + trail, never inlines (§9.4)>"
```

Notes grounded in verified facts:
- **`--worktree` vs `--cwd`.** Design v2 §10 shows `--worktree <wi>`; the PRD
  fixes the worktree NAME as `worktree-<project>-<wi>` (FR-9). The daemon
  **creates the git worktree itself** (`worktree.rs`, `git worktree add`) and
  passes its path as `--cwd`, rather than relying on a CLI-managed worktree, so
  the project-qualified name and lifecycle are daemon-owned. *(Judgment call —
  flagged in §17 / report: the PRD/design are ambiguous on whether `--worktree`
  is a `claude` flag that auto-creates a worktree or whether the daemon manages
  it. This SPEC chooses daemon-managed `git worktree add` + `--cwd` because the
  worktree is "the unit of isolation, config, and correlation" (FR-9) and the
  daemon must control its name and cleanup.)*
- **`--permission-mode default`** + a complete `--allowedTools` allowlist
  (NEVER `bypassPermissions` / `--dangerously-skip-permissions`, which
  propagates to every teammate, FR-14). A too-tight allowlist that stalls the
  session in `state:blocked` is detected as a failure (§3.4, FR-14).
- **`--strict-mcp-config`** restricts the session to exactly the
  daemon-provisioned MCP set — which INCLUDES the role MCPs, not only unblock
  (FR-15, design v2 §4).
- **`--allowedTools`** denies `Edit`/`Write` on `.mcp.json` and
  `.claude/settings` (anti-escape, FR-15).

### 5.2 Provisioning per dispatch (FR-10)

The daemon provisions, in the worktree:
1. **the rendered plugin** via `--plugin-dir` (never re-authoring `.claude/`,
   FR-30) — the stub plugin at P0 (§13.3), the real `unblock-render` output
   from P1.
2. **the per-session `.mcp.json`** (`mcpconfig.rs`) — see §5.4.
3. **the brief** (`brief.rs`) — see §9.4. The brief is passed via `-p`, NOT
   written into the worktree as an authoritative artifact.

### 5.3 Session-id capture (FR-11, R-8)

`--bg` **ignores and warns at** a caller-supplied `--session-id` and generates
its own (warn-and-proceed, NOT a hard error). The daemon therefore:
1. parses the `--bg` stdout for the generated session id if present;
2. otherwise, on the next `observe`, correlates the new session in `claude
   agents --json` by `(--name == wi_id, --cwd == worktree_path)` and reads its
   `sessionId`;
3. persists it in `assignments.session_id` + appends to `sessions`
   (`sessionid.rs`).

Re-adoption of a *session* (not a teammate) is by id via `claude attach`/`claude
logs` if needed for observation; teammates never re-adopt (FR-5).

### 5.4 The per-session `.mcp.json`

The daemon renders an `.mcp.json` that spawns the **stdio proxy shim with the
team baked into the spawn args** (FR-20, transport):

```json
{
  "mcpServers": {
    "unblock": {
      "command": "unblock-agentic",
      "args": ["proxy", "--team", "<team>", "--session", "<wi_id>"]
    },
    "<role-mcp-name>": { "command": "...", "args": ["..."] }
  }
}
```

- Only the **lead** session's `.mcp.json` carries the `unblock` proxy entry
  (FR-19); teammates get only their read/role MCPs (Context7 for Research, a
  design-system MCP for frontend Developers) via the worktree project
  `.mcp.json` — **no `unblock` route** (design v2 §4).
- The proxy entry has **no URL, no port, no key** — the shim bridges to the
  daemon's unix socket (§6); the daemon learns the team from the *connection*,
  not from the args being trustworthy (the args only tell the shim which spawn
  identity to present; the daemon binds team to the connection — §6.2). The key
  never appears anywhere in the worktree (NFR-3).

---

## 6. The unix-socket transport (FR-20, FR-20a, transport)

**Why not http-loopback.** An http-loopback-per-path proxy (`…/team/<team>`)
would let an agent with `Bash` run `curl http://127.0.0.1:PORT/team/<otherteam>`
and obtain another team's injected key → cross-team escape → broken
single-writer (FR-20 security rationale). The stdio-shim-over-unix-socket design
closes this: **no listening TCP port, team fixed by the connection, key never
on the wire**.

### 6.1 The socket

One unix domain socket at
`$XDG_RUNTIME_DIR/unblock-agentic/<org_id>.sock` (fallback under the data dir),
created by `serve` with **mode `0600`, owner = the operator** (operator-only
filesystem permissions). All three interfaces (per-team proxy, control-MCP,
hook-sink) bridge to this **one** socket (§4 PRD three-interfaces, Glossary
"Unix-socket transport").

### 6.2 Connection → role/team routing (`route.rs`)

The shim opens the socket and sends a **HELLO** frame declaring its role and
(for the proxy) its spawn args:

```
HELLO { role: "proxy"|"control"|"hook", team?: "<team>", session?: "<wi_id>", pid }
```

The daemon binds the route from **what it can verify about the connection**,
not from a guessable path:
- For `role:proxy`, the daemon validates that an **active assignment** exists
  whose `team`+`session_name(=wi_id)` match the HELLO AND whose `worktree_path`
  matches the connecting process's cwd (read via `/proc/<pid>/cwd` on Linux,
  `proc_pidinfo` on macOS). A HELLO that does not correspond to a live
  daemon-created assignment is **rejected** — an agent cannot fabricate a team
  it was not dispatched as, because the daemon only honours teams it itself
  dispatched (the assignment is the daemon's own record). This is the
  connection-bound routing the PRD requires ("the daemon learns the team from
  that connection, not from a guessable URL path", FR-20).
- For `role:control`, the daemon accepts only if the connecting uid == the
  operator uid (the socket's `0600` perms already guarantee this; the uid check
  is defence-in-depth).
- For `role:hook`, the frame carries `--event` + `--name`; the daemon records a
  `hook_hints` row (§4.5).

### 6.3 Frame protocol (`frame.rs`)

Length-prefixed JSON (4-byte big-endian length + UTF-8 JSON body). After HELLO,
proxy and control connections carry **MCP JSON-RPC** messages verbatim: the
shim is a transparent stdio↔socket bridge for the Claude-side MCP client. The
daemon terminates the MCP server side, injects the key + `project_id` (proxy)
or dispatches a control tool (control), and replies.

### 6.4 Key injection + deny-reshape (proxy route, `deny.rs`)

On a proxy route, for each MCP `tools/call`:
1. The daemon resolves the **per-team append key** for the connection's team
   (from the keychain, §7) and the **`project_id`** for the assignment.
2. It **denies reshape**: any tool name in the reshape set
   `{create, add_dependency, remove_dependency, close, promote, milestones,
   labels}` (FR-18) is rejected with an MCP error on a team route — reshape is
   daemon-only and happens OUTSIDE the proxy (§8). The append surface
   `{comment, set_state, claim}` (plus reads `ready/show/list/get_state/
   get_trail/search/prime`) is allowed.
3. It forwards the call to `://unblock` (`POST /mcp`) with
   `Authorization: Bearer <team-key>` and the server-validated per-request
   `project_id` (PRD §10.4; the backend resolves `project_id` per request and
   `org.Authorize` enforces cross-project containment —
   `apps/api/org/org.go:691`).

The shim **never sees the key** (it is injected daemon-side); the key **never
touches a TCP socket** (NFR-3). `claim` on the append surface is **lead-only**
and the lead claims its OWN area-task (FR-19); the daemon never claims.

### 6.5 Control + hook routes

- **control** route: the daemon implements the control-MCP tools (§12) directly
  — no `://unblock` proxying; verdicts land in `gate_verdicts` (§4.6) /
  `hook_hints` and are applied by the next reconcile pass.
- **hook** route: the daemon records a deduped `hook_hints` row and signals the
  executor (event trigger, §3.1).

---

## 7. Keys & the encore-internal seed (SQ-2, RD-3, FR/identity-1…6)

### 7.1 The key set

| Key | Label | `agent_kind` | Used by | Surface |
|---|---|---|---|---|
| 1 reshape key | `unblock-agentic-daemon` | `custom` | the daemon reconciler | OUTSIDE the proxy — the reshape set (§8) |
| N per-team append keys | `unblock-agentic-<team>` | `claude-code` | the proxy (one per roster team) | the append surface via the proxy (§6.4) |

All keys are issued to the **operator's real user** (`issued_to_user` =
Miguel), never a synthetic principal (RD-3). Two attribution axes: `issued_to_user`
= authority; `api_key_id`/label = actor (NFR-5; the audit FK
`mcp.tool_calls.api_key_id` is nullable `ON DELETE SET NULL`, PRD §10.4 — the
daemon tolerates NULL).

**`claim` is NOT a reshape** — it is a lead-only append action performed via a
per-team append key (FR-18/FR-19). On a gate item, `claimed_by_id` is therefore
set by the lead's claim through the proxy.

> **Backend-grounded correction (flagged §16/§17-1).** The backend `claim`
> writes `claimed_by_id = ClaimerUserID` (the operator's USER id), NOT the
> api_key_id — `apps/api/workitems/workitems.go:2222` (`claimed_by_id = $2`,
> `$2 = req.ClaimerUserID`) and the MCP handler pins `ClaimerUserID =
> identity.UserID` (`apps/api/mcp/handler_claim.go:65`). Actor attribution
> (which TEAM claimed) is carried by `claimed_by_agent` (the `agent_kind`
> string, `:2223`) and by the audit FK `mcp.tool_calls.api_key_id`, NOT by
> `claimed_by_id`. The PRD's phrasing "`claimed_by_id` on a gate item is the
> team's per-team append key" (FR-22, FR/identity-2) is imprecise: `claimed_by_id`
> is the operator USER; the per-team key is the *actor*, recorded on the audit
> row and (as `agent_kind`) in `claimed_by_agent`. This SPEC implements the
> code's actual semantics; the daemon's Approve precondition (close needs
> `claimed_by_id` non-NULL, §9.2) is satisfied identically either way because
> the lead's claim sets the USER id. No backend change needed — only the
> attribution wording is corrected.

### 7.2 The seed (`keys/seed.rs`, FR/identity-5/6)

The seed is an **encore-internal seed** that invokes the EXISTING private
`auth.IssueAPIKey` RPC **service-to-service** (NOT from `cmd/` — `cmd/` cannot
reach a private Encore RPC, E1388-safe; PRD §11 Seed reuse). `IssueAPIKey`
already exists (`apps/api/auth/auth.go:584`) — the seed simply calls it; this is
**NOT** the deferred key-management BFF.

The seed:
1. ensures the operator user + org + project exist (idempotent upsert; local:
   create/reuse; Cloud: use the OAuth user — FR/identity-4);
2. for each of the `1 + N` keys, calls `IssueAPIKey` with:
   - `OrgID` = the operator's org;
   - `IssuedToUser` = the operator's user;
   - `Label` = `unblock-agentic-daemon` / `unblock-agentic-<team>`;
   - `AgentKind` = `custom` (reshape) / `claude-code` (append);
   - **`CallerUserID` = the operator's user** — which **exercises the dormant
     tenant gate** on `IssueAPIKey` (`apps/api/auth/auth.go:617` — when
     `CallerUserID != ""` it enforces caller-owns-OrgID AND IssuedToUser-is-a-member,
     `:620`/`:634`), thereby **closing the otherwise-dormant cross-tenant write
     IDOR** rather than leaving it dormant (PRD §10.4, FR/identity-5);
3. each call returns the raw key ONCE (`IssueAPIKeyResponse.RawKey`,
   `apps/api/auth/auth.go:546`); the seed **prints it once**.

> **Where the seed lives — flagged §16/§17.** `IssueAPIKey` is a `private`
> Encore RPC (`apps/api/auth/auth.go:583`). "Encore-internal seed,
> service-to-service" means a caller **inside the Encore app** (an Encore
> service or an Encore test/exec context) that can reach a private RPC.
> `crates/unblock-agentic` is a Rust binary OUTSIDE the Encore app and **cannot
> call a private RPC directly**. Therefore the runnable seed must be authored
> **in `apps/api/`** (a small Encore service-to-service entry, e.g. an
> `//encore:api private` seed RPC under a new `seed`/`provisioning` service, or
> an `encore exec`-style internal runner) — this is a **required `apps/api/`
> change, out of this SPEC's scope (§16-A)**. The Rust `unblock-agentic seed`
> subcommand is a thin operator-facing FRONT that triggers that encore-internal
> seed and captures the once-printed raw keys for keychain loading; it does NOT
> itself call the private RPC. `apps/api/exitcriteriontest/seed.go` is
> TEST-fixture scaffolding (direct SQL INSERT + a copy of the HMAC helper —
> `apps/api/exitcriteriontest/seed.go:16-23`), NOT a runnable production seed.

### 7.3 Keychain storage (FR/identity-6, NFR-3)

The once-printed raw keys are loaded into the **OS keychain**, NEVER env vars.
Rationale: the spawned `claude --bg` children **inherit the daemon's
environment**, so an env-var key would leak straight to the agents — breaking
the credential-free invariant. The keychain is read by the daemon/proxy **at
use-time** (§6.4) and is never inherited by a child process.

Implementation: the `keyring` crate (macOS Keychain / Linux Secret Service /
Windows Credential Manager). Service name `unblock-agentic`, account
`<org_id>/<label>`. The `unblock-agentic seed` front offers `--store-keychain`
to load each printed key directly.

---

## 8. Single-writer Model-2 — the desired-DAG apply (FR-18, FR-21, RD-4)

### 8.1 The reshape set (canonical, FR-18)

The **daemon-only** reshape set, performed with the reshape key OUTSIDE the
proxy (`unblock/reshape.rs`):

```
{ create, add_dependency, remove_dependency, close, promote, milestones, labels }
```

- `milestones` / `labels` are **reserved** — reshape-key-eligible but NOT wired
  to any v1 team route (FR-18). The SPEC does NOT exercise them at v1.
- `claim` is **NOT** in the reshape set (lead-only append, §7.1, FR-19).
- No team session ever performs a reshape (the proxy denies the whole set on
  team routes, §6.4).

### 8.2 The Decomposition desired-DAG apply algorithm

The Decomposition team (lead = Fernando) produces the **desired DAG as data**
(a set of `{create, add_dependency}` intentions) on its area-task — it does NOT
mutate the graph. The daemon reads that data from the team's artifacts /
comment trail and applies it. The apply is **idempotent + re-read-and-diff each
tick**:

```
fn apply_desired_dag(desired: &DesiredDag, observed: &GraphObservation)
    -> Vec<ReshapeOp>:
  for node in desired.nodes:
      if observed has no item matching node.idempotency_key:   # see below
          emit Create(node)
  for edge in desired.edges:
      if observed has no edge (from,to,kind):
          emit AddDependency(edge)
  for edge in observed.daemon_authored_edges:
      if edge not in desired.edges:
          emit RemoveDependency(edge)        # only edges the daemon authored
  # Create is cycle-checked inline by the backend; entire create rejected on
  # cycle (catalogue.json `create`: "Cycle-checked inline").
```

**Idempotency key.** The backend `create` has no client-supplied idempotency
token (verified — `apps/api/workitems/workitems.go:920` `Create` mints a fresh
ULID). To make `apply_desired_dag` idempotent across ticks, the daemon
correlates desired nodes to observed items by a **stable natural key it
controls**: `(project_id, title, parent_id)` plus a `unblock-agentic:dag-key=<hash>`
marker written as a `kind=general` comment on create. A desired node already
present (by marker) is NOT re-created. *(Judgment call — flagged §17: the
backend offers no create-idempotency token, so the daemon needs an external
correlation key. The marker-comment approach is Law-3-clean — no schema change.
An alternative — a backend idempotency token on `create` — is flagged as an
optional backend enhancement in §16-B, not required for v1.)*

`remove_dependency` is emitted ONLY for edges the daemon itself authored
(tracked by a `dag-key` marker), so the daemon never removes a human-drawn or
gate edge.

### 8.3 Daemon-owned `promote` (FR-21)

Each tick the daemon promotes (reshape key, outside the proxy):
- newly-created-unblocked items: `Backlog` + `is_ready=true` → `promote` →
  `Ready` (backend `Promote` precondition is exactly `Backlog AND is_ready`,
  `apps/api/workitems/workitems.go:2367`);
- dependents that became `is_ready` after a blocker closed — the native cascade
  recovers `Blocked → Ready` but NEVER `Backlog → Ready`, so the daemon's
  promote is the only path that lifts a `Backlog` dependent (FR-21, design v2
  §8).

---

## 9. Gates, needs-input, findings (FR-22…FR-28, RD-2, RD-5, SQ-3)

### 9.1 GatePending (FR-22)

A human gate IS the phase-boundary work item; the team is dispatched directly
on it; downstream is blocked by a DAG edge. The lead (append, via proxy):
1. **claims** the gate item (`claim` — sets `claimed_by_id` = operator user,
   `claimed_by_agent` = team's agent_kind, status → `InProgress`);
2. sets `impl_state=done` (`set_state`); — this is allowed because
   `claimed_by_id` is now non-NULL (the `impl_done_requires_claim` invariant,
   `apps/api/workitems/workitems.go:1812`);
3. posts `kind=completed, status=success` to the trail.

The daemon projects **GatePending** once that completed-artifact signal is
present (verified via `get_trail` + `get_state`, never self-report) and
`Notify`s the operator (§12.2). GatePending is projected from "gate item +
artifact-complete" — **no new backend state** (FR-22).

### 9.2 Approve (FR-23) — the formal-close sequence

On an operator `approve` verdict (from `gate_verdicts`, §4.6), the daemon (its
reshape key, OUTSIDE the proxy) does EXACTLY:

```
set_state(item, review_state=approved)   # requires impl_state=done — already set by the lead (§9.1)
close(item)                              # requires only claimed_by_id — already set by the lead's claim
```

Grounded preconditions:
- `review_state → approved` requires `impl_state=done`
  (`apps/api/workitems/workitems.go:1845`: `newReview == reviewApproved &&
  newImpl != implDone` → reject). The lead set `impl_state=done` in §9.1, so
  this passes.
- `close` requires ONLY `claimed_by_id IS NOT NULL`
  (`apps/api/workitems/workitems.go:1969`); it does NOT require impl/review/qa.
  The lead's claim set `claimed_by_id`, so this passes.

There is **no `promote → claim` prefix** (the daemon never claims, FR-18/19) and
**no `qa_state=passed` step** (close never requires qa, FR-23). The `close`
fires the **native cascade** (`apps/api/workitems/workitems.go:2014` Regime A
inline `is_ready` recompute + the `CascadeRequested{Reason:"close"}` publish)
that unblocks `Blocked → Ready` dependents; the daemon then `promote`s any
dependent left in `Backlog` (§8.3, FR-21). The dependency edge STAYS (no
`remove_dependency`).

### 9.2.1 Reject (FR-24) + Waive (FR-25)

- **Reject** = keep the gate OPEN; append `kind=review, status=warning`; the
  rework counter increments (§9.5); **re-dispatch** the area-task. `3× →
  escalate to the human` (§9.5).
- **Waive** = the Approve sequence (§9.2) PLUS a `severity=risk` finding
  recording the waived condition (a `type=finding` item via `create` with the
  risk severity, §9.4 findings + root §6.6).

### 9.3 NeedsInput (FR-22, FR-26, RD-5)

NeedsInput is an agent genuinely stuck mid-task — a DISTINCT signal from
GatePending, never sharing a column (FR-22). The **lead** sets it (append):
`pipeline_state=needs_human` via `set_state` + a `kind=needs-human,
status=warning` comment carrying the question. The daemon reads it.

- `set_state` can write `pipeline_state` unconditionally (verified —
  `apps/api/workitems/workitems.go:1804` `newPipeline = coalesceState(...)`
  with no precondition on the pipeline column; a pure pipeline write is
  accepted and publishes a cascade, `:1866`).
- `pipeline_state=needs_human` is a valid enum value (`running | needs_human |
  paused | no_investigation`, PRD §10.4).
- The `status=warning` on the `needs-human` comment is the **deliberate
  divergence** the PRD mandates ("a question is a warning, not an error") — the
  SPEC preserves it and does NOT "correct" it to `error` (FR-22).

**Resolution (FR-26):** the operator answers via the control-MCP `answer` tool
(§12). The daemon: posts the answer to the trail (`comment`), sets
`set_state(pipeline_state=running)`, and **re-dispatches** the area-task with
the answer in the brief (§9.4).

### 9.4 Brief composition (`brief.rs`, FR-10, §17-3)

The brief is composed from `://unblock` reads and is passed via `-p`. It MUST
**point to** the spec/plan and the comment trail and **NEVER inline an
authoritative copy** (the bead-description-is-not-the-spec rule). Brief schema:

```
Brief {
  identity:   { team, role, area_kind, persona }     # who you are
  target:     { project_id, wi_id, title, type }      # the area-task (from `show`)
  pointers:   {                                        # POINTERS, never inlined copies
     spec_path:  "docs/specs/NN-spec-*.md#section",    # path + anchor, from the item body/labels
     plan_path:  "docs/plans/NN-plan-*.md#section",
     parent_id:  "<epic id>",                          # read the parent for context
     trail_hint: "read get_trail(wi_id) for the full comment trail",
  }
  mode:       "lens" | "partition"                     # §8.2/§8.3 PRD
  partition?: { file_globs: [...] }                    # Developers only, §9.6 / §17-2
  answer?:    "<the needs-input answer>"               # only on a NeedsInput re-dispatch
  rules:      [ "Only the lead talks to ://unblock (append-only).",
                "Verify against artifacts + trail, never self-report.",
                "Do not edit .mcp.json or .claude/settings." ]
}
```

The brief carries **paths + ids + an instruction to read the trail**, not the
spec text. The daemon obtains `title/type/parent_id` from `show`/`get_state`
and the `spec_path`/`plan_path` from the item body or a dedicated label; if the
item does not carry a spec pointer, that is a dispatch precondition failure (the
daemon surfaces NeedsInput rather than dispatching a brief with no authoritative
target — §17-3).

### 9.5 Rework counter (FR-27, §17-4)

The counter is **derived from the comment trail each pass** — Law-3-clean, no
schema change. The counting rule:

> count of `kind=review` comments with `status ∈ {error, warning}` on the item.

Grounded: `kind=review` and `status ∈ {error|warning|info|success}` are valid
(PRD §10.4; `comments_kind_chk` / `comments_status_chk`,
`apps/api/mcp/handler_set_state.go:186-208`). The daemon reads the trail
(`get_trail`), counts matching comments, and writes the count to
`rework_counters.count` (a cache, §4.3). When the count reaches **3**, the
daemon **escalates to the human** (a control-MCP `pending` entry of kind
`rework-escalation` + an OS notification) and sets `rework_counters.escalated=1`
so the escalation fires exactly once.

> **Note (§17-4):** Reject (FR-24) appends `kind=review, status=warning`, so the
> Reject path is what increments this counter. The PRD's FR-27 says "count
> `kind=review` with `status=error`/`warning`"; the Waive/Approve `severity=risk`
> finding is a SEPARATE `type=finding` item, not a `kind=review` comment, so it
> does not increment the rework counter — correct and intended.

### 9.6 Findings topology (FR-28, SQ-3 — severity-driven, no auto-edges)

Findings are routed by SEVERITY, reusing the root PRD severity catalogue (root
§6.10/§6.11). **The daemon NEVER auto-creates a blocking edge from a finding.**

- **High severity (CRITICAL review / BLOCKER QA)** → rework via STATE, not a
  separate item: write `review_state=needs_rework` (or a qa-fail) and
  **re-dispatch** the originating area-task (the FR-24 Reject / rework-counter
  machinery). `review_state=needs_rework` auto-resets `qa_state=pending`
  (invariant I-1, `apps/api/workitems/workitems.go:1807`). No separate finding
  item, no edge.
- **Lower severity (WARNING / MAJOR / RISK / SUGGESTION / MINOR / DEVIATION /
  EXTRA)** → a `type=finding` item linked via `discovered_from_id` + `parent_id`
  (root §6.6), **INFORMATIONAL by default — no blocking edge** — scheduled as
  independent work (it enters the ready queue once created/promoted).

Blocking arises ONLY from (a) a human-gate **Reject** verdict (FR-24, keeps the
gate open) or (b) the dependency edges the **Decomposition** team drew (§8.2).
Law 1 stays clean: blocking structure is authored deliberately, never
synthesised reactively from findings.

---

## 10. Concurrency budget (FR-32…FR-34, §17-5)

Two-dimensional (SQ-1):
- an **org-wide ceiling** (`org_max_concurrent`, the fleet-level cap);
- a **per-project quota** (`max_agents_per_project`, per-project fairness so one
  busy project cannot starve others).

### 10.1 `inflight` recomputation (FR-34)

`inflight` is **recomputed from `claude agents --json` each pass**, never a
running counter — a missed `Stop` must not leak a slot. `inflight_org` =
count of live background sessions whose `--name` maps to a daemon assignment;
`inflight_project[p]` = the subset whose assignment is in project `p`.

### 10.2 Dispatch-admission rule (FR-32, §17-5)

A `Ready`, unclaimed, unassigned item is admitted for `Dispatch` only if BOTH:
```
inflight_org             < org_max_concurrent
inflight_project[proj]   < max_agents_per_project
```
Admission consumes the ready frontier in the **backend ready-queue order**:
`(priority asc, created_at asc, id asc)` — the exact ordering of the `ready`
tool (`catalogue.json` `ready`: "ordered by (priority asc, created_at asc, id
asc)", `apps/api/workitems/workitems.go:2589` `Ready`). The daemon does NOT
reimplement topo-scheduling; it reads `ready` per project and admits in that
order until a ceiling/quota is hit. Admitted items get a `budget_reservations`
row (§4.4) within the pass (crash-safe slot accounting).

### 10.3 Surplus + no preemption (FR-33)

When a ready frontier exceeds the org ceiling OR a project exceeds its quota,
surplus items **stay `Ready`** (an implicit queue ordered by priority +
critical-path depth, fair-shared across projects). **No preemption.**

### 10.4 Phasing

At P1 the org ceiling + per-project quota are enforced as the admission gate
above. The full budget POLICY (agent-slot accounting beyond session counting,
the fairness *scheduler* across projects, reserve, backpressure signals) is a
P3 deliverable (FR-32, §14).

---

## 11. Degraded mode (NFR-7, SQ-5, Law 3)

When `://unblock` is unreachable, the daemon enters degraded mode
(`degraded.rs`): **pause + fail-fast + backoff + re-converge from truth**.

- **Pause** new dispatches; **retry the reconcile tick with exponential
  backoff** (e.g. 15s → 30s → … capped at e.g. 5 min, with jitter).
- **Fail-fast** in-flight backend writes — **NO buffering, NO replay**: a write
  that didn't land = didn't happen; the affected area-task is simply
  *incomplete*. (This supersedes any "buffer/replay" framing — there is no
  buffering.)
- **Re-converge from truth on recovery**: the reconciler re-reads truth,
  re-diffs, and re-dispatches incomplete area-tasks (the restart invariant
  generalised — level-triggered). It **NEVER advances state on assumption**.
- The degraded state is **surfaced via the control-MCP** (`status` tool +
  notification) so the operator sees it in the chat.

Reachability is probed by a cheap read (`prime` or `ready` with limit 1) at the
top of each pass; an error (connection refused, 5xx, timeout) trips degraded
mode; a subsequent success clears it and forces a re-converge pass.

---

## 12. The control-MCP (FR-20a, SQ-4)

The operator's surface for gates, needs-input, and status is the daemon's
**local control-MCP**, consumed by the operator's OWN Claude Code session
(chat-first). The operator's `.mcp.json` spawns `unblock-agentic ctl`, which
bridges to the operator-only unix socket (§6, control route).

### 12.1 Tools

| Tool | Args | Effect |
|---|---|---|
| `status` | `{project_id?}` | snapshot: inflight per project, ready frontier, degraded state, open gates/needs-input |
| `pending` | `{}` | the list of items awaiting a human verdict: GatePending, NeedsInput, rework-escalations |
| `approve` | `{project_id, wi_id}` | record an `approve` verdict (§4.6) → applied next pass (§9.2) |
| `reject` | `{project_id, wi_id, note?}` | record a `reject` verdict → §9.2.1 |
| `waive` | `{project_id, wi_id, condition}` | record a `waive` verdict → §9.2.1 (approve + risk finding) |
| `answer` | `{project_id, wi_id, body}` | record a NeedsInput answer → §9.3 resolution |

The control tools are **write-to-ledger** (`gate_verdicts`, §4.6), NOT direct
backend mutations: the reconciler applies them on the next pass, preserving the
single idempotent act path and the truth-precedence rule. (`status`/`pending`
are pure reads of observed state + ledger.)

### 12.2 OS notifications (`control/notify.rs`)

The daemon cannot push into a running Claude session, so it fires **OS desktop
notifications** when attention is needed (a new GatePending, a NeedsInput, a
rework escalation, or entering degraded mode). macOS via `osascript`/
`NSUserNotification`-equivalent (the `notify-rust` crate; macOS/Linux/Windows
backends). The notification text points the operator at the chat ("`ask Claude:
pending`").

### 12.3 Optional HTML dashboard (post-v1, §12.3)

A secondary loopback HTML view (`notify/dashboard.rs`) is **post-v1** and
OPTIONAL; `apps/web` integration is post-v1 (would pull the surface onto the
v1.1 web path). Not implemented at v1.

---

## 13. Renderer relationship + Law-8 survival (FR-30, FR-31, T1, T3)

### 13.1 Delegation (T1, FR-30)

The daemon delegates ALL Claude Code configuration to `unblock-render` via
`--plugin-dir <rendered plugin>`. It NEVER re-authors `agents/`, `skills/`,
`hooks/`, or the MCP config. Split: renderer = WHAT config to write; daemon =
WHEN/WHERE + spawn + reconcile.

### 13.2 Additive Stop hook (T3, FR-17, FR-31, §17-8)

The daemon's hook-sink is **additive** to the renderer's `verify-state` Stop
hook. Claude Code dedupes Stop hooks **by command string**, so both fire only
if the commands DIFFER. The renderer's hook runs `verify-state`; the daemon's
Stop hook MUST run a **distinct command** — the hook shim
`unblock-agentic hook --event Stop --name <wi>` (§6.5). Because the command
string differs from `verify-state`, both Stop hooks fire: the renderer's
Layer-2 `verify-state` finding still emits, AND the daemon's sink records its
hint. The daemon dispatches renderer-produced personas, so Layer 3 (the
personas' BLOCK conditions) survives. Layer 1 (the backend state-transition
validator) is parallelism-immune.

### 13.3 The stub plugin (P0, §13.3, R-5)

A **conformant, NOT throwaway** packaged plugin at
`crates/unblock-agentic/data/stub-plugin/`, shaped to the `unblock-render`
output contract (commit `69dad9a`):
- `.claude-plugin/plugin.json` — a valid manifest;
- `agents/` — at least the dispatched **coordinator persona** AND **every
  teammate persona** the Lens/Partition spawns reference;
- `hooks/` — a `verify-state`-equivalent Stop hook (so §13.2's distinct-command
  obligation is meaningfully testable against it).

Sufficient to dispatch a real lead+teammates team in P0/P1. The P1 cutover to
the real `unblock-render` output is **config-only** (`--plugin-dir` repoint),
because the daemon never re-authors `.claude/` (FR-30).

---

## 14. Phasing (P0–P3)

`unblock-agentic`'s ladder is **P0–P3**, distinct from `://unblock`'s
`P01–P05`. Substrate is local `encore run` (`127.0.0.1:9900`, app id
`unblock-sco2`) for P0–P2; cutover to Encore Cloud at P3, gated on the backend
deploy (E-1 / `unblock-8xb.5.1`) — NOT deployed today (R-6).

| Phase | Scope | Modules | Substrate |
|---|---|---|---|
| **P0** | Scaffold the crate; ledger (§4) + `reconcile()` (§3) + unix-socket transport (§6) + proxy shim + hook-sink + the encore-internal seed (§7) + the conformant stub plugin (§13.3). SM-3 (restart recovery) HARD-gates P0. | `daemon/`, `ledger/`, `transport/`, `proxy/`, `hooksink/`, `keys/`, `data/stub-plugin/` | local `encore run` |
| **P1** | Developers end-to-end (one Area: dispatch → team → findings → completion → DAG propagation). Concurrency ceiling + per-project quota + admission (§10). `--plugin-dir` cuts over from the stub to real `unblock-render` output. SM-1 (parallel speedup, SOFT) + SM-2 (no self-report advancement, HARD) gate P1. | `dispatch/`, `unblock/`, `daemon/lifecycle.rs`, `daemon/plan.rs` (Developers route), budget | local `encore run` |
| **P2** | Full 8-team roster + gates + the chat-first control-MCP (§12): GatePending vs NeedsInput, status/pending/approve/reject/waive/answer + OS notifications. Optional HTML dashboard is post-v1. | `control/`, gate logic (§9), `daemon/act.rs` gate actions | local `encore run` |
| **P3** | Budget / agent-slots policy + backpressure + `rtk gain` tuning. Cutover to Encore Cloud once the backend is DEPLOYED. | budget policy, fairness scheduler | Encore Cloud |

---

## 15. Manifesto-Law mapping

| Law | How `unblock-agentic` respects it |
|---|---|
| **L1 — Cascade is structural** | The daemon fires `close` (§9.2); the native cascade (`workitems.go:2014` + `CascadeRequested{close}`) unblocks dependents; the daemon `promote`s `Backlog` dependents (§8.3). |
| **L2 — One graph, one truth** | Scheduling reads readiness from `://unblock`'s `ready` oracle (§10.2); the daemon reconciles to the graph, never a private topology. |
| **L3 — Postgres is the source of truth** | The ledger (§4) is orchestration-only, rebuildable from truth; on conflict `://unblock` + `agents --json` win over hints (§3.3); backend-unreachable = degraded mode (§11), never worked around. |
| **L4 — BFF is structural** | The daemon holds keys (§7) and POSTs server-side; agents are credential-free; the report surface is the chat-first control-MCP (§12), no browser credential. |
| **L5 — Structured project memory** | Out of scope at v1 — the daemon does not author `memory.*` entries. |
| **L6 — Decoupled deliverables share no runtime state** | Zero shared runtime state with `unblock-code`; the data plane is exclusively `://unblock` via MCP (§17-6). |
| **L7 — Provider-agnostic** | Inherited from the substrate; the daemon adds nothing provider-specific. |
| **L8 — Pipeline gates enforced architecturally** | Additive Stop hook with a distinct command (§13.2); dispatches renderer-produced personas; never re-authors `.claude/`; all three layers survive parallel orchestration (§18 test T-LAW8). |

---

## 16. Required backend changes / missing capabilities

These are capabilities the daemon needs that `apps/api/` does NOT provide today.
They are **out of this SPEC's scope** (they belong to `apps/api/` work) and are
flagged for the human:

- **§16-A (required) — a runnable encore-internal seed entry.** `IssueAPIKey`
  is `private` (`apps/api/auth/auth.go:583`) and unreachable from a Rust binary
  outside the Encore app. A small **`apps/api/` service-to-service seed** (a new
  `//encore:api private` seed RPC under a `seed`/`provisioning` service, or an
  `encore exec` internal runner) is required so the operator can mint the
  `1 + N` keys with `CallerUserID` pinned. The `unblock-agentic seed` subcommand
  is only the operator-facing FRONT. (PRD §11 calls this "the encore-internal
  seed" but does not specify WHERE it lives; this SPEC pins it to `apps/api/`.)
- **§16-B (optional enhancement, not required) — a `create` idempotency token.**
  The backend `create` mints a fresh ULID with no client idempotency token
  (`workitems.go:920`), so the daemon uses an external `dag-key` marker comment
  to make `apply_desired_dag` idempotent (§8.2). A backend idempotency token on
  `create` would let the daemon drop the marker; it is NOT required for v1.
- **§16-C (none) — single-writer is intentionally NOT a backend guarantee.**
  `org.Authorize` does not branch on `agent_kind`
  (`apps/api/org/org.go:708-716`); single-writer stays an orchestration
  convention (NFR-4). No backend change is requested here — this is recorded so
  it is not mistaken for a gap.

No other backend capability is missing for v1: `set_state(pipeline_state=…)`,
`close` (claimed-only), `review_state=approved` (impl-done), `promote`
(Backlog+is_ready), `claim` (lead), the 23-tool surface, and the audit FK are
all present and grounded above.

---

## 17. Resolution of the 8 open §13 PRD SPEC-TODOs

**17-1. `needs_human` (agent-stuck, lead-set) vs `paused` (operator-paused,
control-set) — the convention.** The backend enforces NEITHER (a pure
`pipeline_state` write is unconditional, `workitems.go:1804`). The SPEC defines
the convention:
- `pipeline_state=needs_human` is set **only by a team LEAD** (via the proxy
  append surface) to signal NeedsInput (§9.3), always paired with a
  `kind=needs-human, status=warning` comment carrying the question.
- `pipeline_state=paused` is set **only by the daemon** (reshape key, on an
  operator `pause` intent) to mark an operator-paused area-task. *(Judgment call
  — flagged: the PRD names `paused` as "operator-paused, control-MCP-set" but
  the control-MCP tool list (§12.1) has no `pause` tool. This SPEC RESERVES
  `paused` for a daemon-set operator-pause and does NOT add a `pause` control
  tool at v1 — pausing the whole fleet is the degraded/operator-stop concern,
  not a per-item control verb. If per-item operator-pause is wanted, add a
  `pause`/`resume` control tool — flagged for human review.)* The two states
  never share the NeedsInput projection: NeedsInput is projected ONLY from
  `needs_human` + the `needs-human` comment; `paused` suppresses dispatch but is
  not a NeedsInput.

**17-2. Developers/Partition file slices in one shared worktree.** Mechanism:
the Developers lead owns the single shared worktree (`--cwd`); teammates share
that cwd (NOT N worktrees, §8.3 PRD). The lead assigns **disjoint file globs**
per teammate in the spawn prompt (the `partition.file_globs` brief field, §9.4)
— e.g. teammate A owns `apps/api/foo/**`, teammate B owns `apps/api/bar/**`.
Conflict avoidance is by the disjoint-glob assignment; the **lead reconciles**
the slices (it is the only writer to `://unblock` and the synthesiser of the
team's output). There is no in-team advocate; the adversarial check comes
downstream from Review + QA. *(Judgment call — flagged: Claude Code does not
mechanically enforce file-glob ownership among teammates sharing a cwd; this is
a coordination convention the lead enforces via the mailbox, NOT a hard
guarantee. Hard enforcement would need per-teammate worktrees, which the PRD
explicitly rejects (§8.3). Flagged so the human accepts the convention-not-guarantee
posture, consistent with single-writer being a convention.)*

**17-3. Brief schema (points, never inlines).** Resolved in §9.4: the brief
carries `pointers` (`spec_path#anchor`, `plan_path#anchor`, `parent_id`, a
"read `get_trail`" hint) + identity + target ids + mode + optional partition
globs + optional needs-input answer + the standing rules. It NEVER inlines the
spec/plan text (bead-description-is-not-the-spec). An item with no spec pointer
is a dispatch precondition failure → NeedsInput, not a dispatch with an
authoritative-copy brief.

**17-4. Rework-counter persistence.** Resolved in §9.5: count `kind=review`
comments with `status ∈ {error, warning}` on the item, derived from
`get_trail` each pass (Law-3-clean, no schema change); cached in
`rework_counters` (§4.3) for once-only escalation; `count == 3` → escalate to
the human (control-MCP `pending` + OS notification), deduped by
`rework_counters.escalated`.

**17-5. Slot accounting at P1.** Resolved in §10: org-wide ceiling +
per-project quota; `inflight` recomputed from `agents --json` each pass;
admission consumes the ready frontier in the backend `ready` order `(priority
asc, created_at asc, id asc)`; surplus stays `Ready`, no preemption. Full
budget policy/fairness scheduler is P3.

**17-6. Root FR-27 scoping.** The "no daemon / no watcher" invariant is
**AST-CLI-only** — it constrains `unblock-code` (the local one-shot indexer),
NOT this product. `unblock-agentic` shares zero runtime state with
`unblock-code` (Law 6); its data plane is exclusively `://unblock` via MCP. The
needed one-line clarification belongs in the ROOT PRD (`docs/PRD.md` FR-27),
e.g. *"This 'no daemon / no watcher' invariant scopes the AST CLI
(`unblock-code`) only; the post-v1 `unblock-agentic` control-plane daemon is a
separate product and is not constrained by it (Law 6: zero shared runtime
state)."* **Flagged as a root-doc edit — NOT made here** (this SPEC's only
root-doc touch is the §1 pointer in `docs/SPEC.md`, per the doc-only
constraint). Reported to the human to apply to `docs/PRD.md` FR-27.

**17-7. Worktree threat-model.** The proxy is a **trust boundary, not a security
boundary** (NFR-3, R-3). Blast radius if a worktree somehow obtained a direct
key: bounded precisely because the design is **credential-free** — the key
never enters the worktree (kept in the keychain, read daemon-side at use-time,
§7.3), never in env (children inherit env, §7.3), never on a TCP socket (§6),
never seen by the shim (§6.4). The append key is scoped to the append surface
only (the proxy denies reshape, §6.4), so even a leaked append key cannot
reshape the graph; and `org.Authorize`'s agent branch is read+write on a fixed
resource set within the SAME org only (`org.go:708`), so a leaked key cannot
cross tenants. Worktree isolation is the real containment; the credential-free
posture caps the blast radius to "one team's append surface within one org."

**17-8. Layer-2 integration test under orchestration.** Resolved as test
**T-LAW8** (§18): an integration test that, under daemon orchestration, drives a
pipeline-bypass and asserts the renderer's Layer-2 `verify-state` finding STILL
emits. The test asserts the daemon's Stop-hook command string
(`unblock-agentic hook --event Stop …`) **differs** from the renderer's
(`verify-state`), so Claude Code's command-string dedup fires BOTH hooks
(§13.2). The stub plugin (§13.3) ships a `verify-state`-equivalent Stop hook so
the test is runnable in P0 before the real renderer lands.

---

## 18. Test obligations & acceptance

Quality gates per `crates/` (CLAUDE.md): `cargo fmt --check --all`, `cargo
clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`,
`cargo doc --no-deps`.

| ID | Test | Gates | Asserts |
|---|---|---|---|
| **T-RECONCILE-IDEMPOTENT** | run `reconcile()` twice with no change | P0 | second pass emits zero Actions (FR-2) |
| **T-RESTART** (SM-3, HARD) | `kill -9` + next tick | P0 | recovers in-flight from `agents --json` + `://unblock` + ledger within one pass; no leaked slot, no double-dispatch, no lost gate (FR-5, NFR-1) |
| **T-SESSIONID** | dispatch with a supplied `--session-id` | P0 | daemon captures the `--bg`-GENERATED id, correlated by `--name`/`--cwd`, persisted (FR-11, R-8) |
| **T-TRANSPORT-ROUTE** | proxy HELLO with a fabricated team | P0 | rejected — no matching daemon assignment (FR-20) |
| **T-DENY-RESHAPE** | a team route calls `create`/`add_dependency`/`close`/`promote` | P0 | denied at the proxy (FR-18, §6.4) |
| **T-NO-KEY-IN-WORKTREE** | inspect the worktree + child env after dispatch | P0 | no raw key in `.mcp.json`, the worktree, or the child env (NFR-3, §7.3) |
| **T-SEED-IDOR** | seed with `CallerUserID` pinned vs a foreign org | P0 | foreign org → NOT_FOUND (the gate fires); same org → key minted (FR/identity-5; `auth.go:617`) |
| **T-NO-SELF-REPORT** (SM-2, HARD) | a confabulated "done" with no artifact | P1 | item NEVER advanced; advancement requires `kind=completed` + verified artifact (FR-13, NFR-2) |
| **T-ADMISSION** | ready frontier > ceiling and > quota | P1 | admits in `(priority,created_at,id)` order up to org ceiling + per-project quota; surplus stays Ready; no preemption (FR-32/33) |
| **T-INFLIGHT** | drop a `Stop` event | P1 | `inflight` recomputed from `agents --json`, no leaked slot (FR-34) |
| **T-APPROVE** | operator approve on a GatePending | P2 | exactly `set_state(review_state=approved) → close`; cascade unblocks dependents; daemon promotes Backlog dependents (FR-23, §9.2) |
| **T-REJECT-3X** | three rejects | P2 | rework counter (trail-derived) hits 3 → escalate-to-human fires once (FR-24, FR-27, §9.5) |
| **T-NEEDS-INPUT** | lead sets `needs_human` + `needs-human` comment | P2 | daemon projects NeedsInput (distinct from GatePending); `answer` posts the answer + `pipeline_state=running` + re-dispatch (FR-22/26, §9.3) |
| **T-FINDINGS** | high-sev vs low-sev finding | P1/P2 | high-sev → rework via state (no item, no edge); low-sev → `type=finding` item, informational, no blocking edge; NO daemon-authored blocking edge ever (FR-28) |
| **T-DEGRADED** | `://unblock` unreachable | P0/P1 | pause dispatch + backoff + fail-fast (no buffer/replay) + re-converge on recovery; surfaced via control-MCP (NFR-7) |
| **T-LAW8** (§17-8) | pipeline-bypass under orchestration | P0+ | renderer's Layer-2 `verify-state` finding STILL emits; daemon Stop-hook command differs from `verify-state`; both fire (FR-17, T3) |

**Acceptance.** P0 done when T-RESTART (SM-3) passes plus the P0-row tests; P1
done when T-NO-SELF-REPORT (SM-2) passes plus the P1-row tests; P2 done when the
gate/needs-input/control-MCP tests pass; P3 done when the budget policy +
Cloud-cutover tests pass.

---

## 19. Traceability

Every section maps back to a PRD requirement: §3↔FR-1–5, §3.4↔FR-4, §3.5↔FR-5/SM-3,
§4↔FR-12/NFR-1, §5↔FR-9–15/R-8, §6↔FR-16/FR-20/FR-20a/transport, §7↔FR/identity-1–6/SQ-2,
§8↔FR-18/FR-21/RD-4/RD-6, §9↔FR-22–28/RD-2/RD-5/SQ-3, §10↔FR-32–34/SQ-1, §11↔NFR-7/SQ-5,
§12↔FR-20a/SQ-4, §13↔FR-30/31/T1/T3, §14↔PRD §11, §15↔PRD §10.3, §17↔PRD §13 (8 TODOs),
§18↔SM-1/2/3 + the FRs. Backend facts are grounded in `apps/api/` at the `file:line`
citations inline.
