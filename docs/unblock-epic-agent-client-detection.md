# Unblock — Epic 1.5: Agent Client Detection

**Dependency-aware task tracking for AI agents, powered by GitHub.**

| | |
|---|---|
| **Epic** | 1.5 — Agent Client Detection |
| **Version target** | v0.1.0 (Phase 1) |
| **Author** | Miguel Ramos |
| **Org** | websublime |
| **Date** | March 2026 |
| **Status** | Draft |
| **Parent plan** | `unblock-project-plan.md` |

---

## Motivation

The MCP server accepts connections from any MCP-compatible client: Claude Code, GitHub Copilot,
Cursor, Cline, Aider, and others. Currently, all clients are treated identically — the server
has no awareness of who is calling.

Knowing the client matters for:

- **Structured logging** — `agent.client` as a span field enables filtering logs by client in
  production (e.g., "how many Claude Code sessions vs Copilot today?")
- **`prime` tool enrichment** — session context should include which AI is running the session,
  making the output more useful for supervisors and audit trails
- **`stats` aggregation** — future: sessions per client, tool usage by client type
- **Telemetry / OpenTelemetry** — the ARCH §14.3 metric budget includes tool-level attribution;
  `agent.client` is a natural dimension
- **Debugging** — when an agent behaves unexpectedly, knowing whether it was Claude vs Copilot
  vs Cursor narrows the root cause

The MCP protocol provides a native, zero-cost hook for this: the `initialize` handshake. Every
client sends `clientInfo { name, version }` before calling any tool. No extra API calls, no
polling, no storage.

---

## Architectural Placement

| Concern | Crate | Module |
|---|---|---|
| Domain types (`AgentKind`, `AgentClient`) | `unblock-core` | `client` (standalone module) |
| Detection logic (`ClientDetector`) | `unblock-core` | `detection` (standalone module) |
| MCP `initialize` capture + `AgentKind` storage | `unblock-mcp` | `server` (`ServerState.agent_kind`) |
| `SessionMeta` in `prime` output | `unblock-mcp` | `tools::prime` |
| Span fields | `unblock-mcp` | all tool handlers |

This follows the existing layering: `unblock-core` owns domain types and pure logic;
`unblock-mcp` owns runtime behaviour, I/O, and MCP protocol integration.

**File structure:** `client.rs` and `detection.rs` are standalone modules in `unblock-core/src/`,
following the same pattern as `errors.rs`, `config.rs`, and `cache.rs`. No directory restructuring
of `types.rs` is needed.

---

## Design Decisions

### D1: Detection priority — MCP first, env fallback, Unknown last

```
clientInfo (initialize) → env vars → Unknown("unknown")
```

MCP `clientInfo` is authoritative. Env vars cover the case where a client doesn't populate
`clientInfo` correctly (e.g., a custom wrapper around Claude Code that forwards the MCP
protocol but doesn't re-set the client name). `Unknown` is a valid, non-fatal state — the
server always starts regardless of whether detection succeeds.

### D2: `AgentKind` is informational, never behavioural

The detected client **does not change tool behaviour**. No conditional branching on `AgentKind`
inside tool handlers. This keeps the server predictable and avoids a maintenance surface.
Detection is purely for observability and surface enrichment (logging, `prime`, telemetry).

If client-specific behaviour is ever needed, it belongs in a separate ADR.

### D3: `AgentKind` lives in `unblock-core`, not `unblock-mcp`

`AgentKind` is a domain concept (which kind of agent is using the system) not an MCP transport
concept. Placing it in `unblock-core` makes it available to the desktop app, future CLI tools,
and test harnesses without pulling in MCP dependencies.

### D4: Compound `Agent` field is not extended

The `Agent` Projects V2 field keeps its current format: `username:supervisor`. Adding the
client kind (e.g., `miguelramos:rust-supervisor:claude-code`) would pollute the field that
humans read in the GitHub board. Client kind belongs in logs and telemetry, not in GitHub data.

### D5: Resolve once in `initialize`, store in `ServerState` via `OnceLock`

rmcp's default `initialize` handler already stores `InitializeRequestParams` in
`Peer<RoleServer>` (accessible via `peer.peer_info()`). However, we override `initialize` to:

1. **Resolve `AgentKind` once** at session start (not per-tool-call)
2. **Emit a `tracing::info!` event** with client name, version, and kind at connection time
3. **Store in `ServerState.agent_kind: OnceLock<AgentKind>`** so tool handlers read from
   shared state without importing rmcp-specific types

This is a hybrid approach: we use rmcp's built-in peer storage for raw `clientInfo`, but
resolve and cache the `AgentKind` ourselves for clean tool handler access.

**Rejected alternative:** Using `Peer<RoleServer>` extraction directly in tool handlers.
This couples every handler to rmcp types and resolves `AgentKind` on every call. The
overhead is negligible, but the coupling is undesirable.

### D6: `VSCODE_PID` dropped from env detection

`VSCODE_PID` is set for **any** VS Code session, not specifically GitHub Copilot. Using it
as a Copilot signal would misidentify users running Cursor in VS Code, or plain VS Code
without Copilot. Only `GITHUB_COPILOT_TOKEN` is used for Copilot env detection.

File-based signals (e.g., `~/.config/github-copilot/`) indicate installation, not runtime
client identity. They are not suitable for detection.

---

## Tasks

### Epic 1.5 — Agent Client Detection

**Goal:** Identify which AI client is connected and surface this in logs, `prime` output, and
tracing spans — without affecting tool behaviour.

**Depends on:** Epic 1.4 (MCP tools foundation), task 1.4.14 (`prime` tool)

| Task | Description | Definition of Done | Ref |
|---|---|---|---|
| **1.5.1** `AgentKind` + `AgentClient` types | In `unblock-core/src/client.rs` (standalone module). `AgentKind` enum: `ClaudeCode`, `Copilot`, `Cursor`, `Cline`, `Aider`, `Unknown(String)`. `AgentClient { name: String, version: String }`. `AgentKind::from_client_name(&str)` — case-insensitive substring match. `AgentKind::as_str() -> &str`. `impl Display for AgentKind`. All types derive `Debug, Clone, PartialEq` | Compiles. Unit tests: known names → correct variant. Unrecognised name → `Unknown(name)`. `Display` emits same string as `as_str()`. `cargo doc` clean | ARCH §5 |
| **1.5.2** `ClientDetector` | In `unblock-core/src/detection.rs` (standalone module). `ClientDetector::from_env() -> Option<AgentKind>` — reads `CLAUDE_CODE_ENTRYPOINT`, `GITHUB_COPILOT_TOKEN`, `CURSOR_TRACE_ID`. `ClientDetector::resolve(mcp_client: Option<&AgentClient>) -> AgentKind` — MCP → env → `Unknown`. Both methods `#[must_use]`. **Note:** `VSCODE_PID` dropped — too broad (any VS Code session, not Copilot-specific). See D6 | Unit tests: each known env var → correct kind. MCP overrides env. Both absent → `Unknown`. No panics, no I/O side effects. Pure function, no `async` | — |
| **1.5.3** MCP `initialize` capture | In `unblock-mcp/src/server.rs`. Add `agent_kind: OnceLock<AgentKind>` to `ServerState`. Override `initialize()` to: extract `client_info`, construct `AgentClient`, resolve `AgentKind` via `ClientDetector::resolve()`, store in `OnceLock`, emit `tracing::info!` with `client.name`, `client.version`, `client.kind` fields. Delegate to rmcp default for `peer_info` storage | Integration test: mock `initialize` with known `clientInfo` → `AgentKind::ClaudeCode` stored in `OnceLock`. Missing `clientInfo` → env fallback. `tracing` event present | ARCH §9 |
| **1.5.4** `SessionMeta` in `prime` output | In `unblock-mcp/src/tools/prime.rs`. Define `SessionMeta { agent_client: String, agent_kind: String, agent_field: Option<String>, connected_at: DateTime<Utc> }`. Add `session: SessionMeta` to `PrimeResult`. Read `AgentKind` from `ServerState.agent_kind` (`OnceLock`). Read raw client name from `Peer<RoleServer>.peer_info()` | Integration test: `prime` response JSON includes `session` object. Fields populated correctly for known client. `agent_field` is `None` when `UNBLOCK_AGENT` not set | PRD §6.3, ARCH §10 |
| **1.5.5** `agent.client` span fields | In every tool handler in `unblock-mcp/src/tools/`. Add `agent.client` and `agent.kind` as fields on the root `tracing::info_span!` for each tool call. Read from `ServerState.agent_kind` | JSON log output includes `agent.client` and `agent.kind` fields on all tool spans. Verified by a log-capture integration test that calls `ready` and asserts field presence. Token not leaked | ARCH §14.1 |
| **1.5.6** Tests | Unit: all `AgentKind` variants (table-driven). `ClientDetector` env + MCP paths. `from_client_name` fuzz: random strings never panic. Integration: full `initialize → prime` flow with mock client | >80% coverage on `client` and `detection` modules. All tests green. `cargo clippy -D warnings` clean | Quality Gate |

---

## Implementation Notes

### 1.5.1 — Type signatures (idiomatic Rust)

```rust
// unblock-core/src/client.rs

/// The kind of AI agent connected to the MCP server.
///
/// Detected from the MCP `initialize` handshake (`clientInfo.name`) or,
/// as a fallback, from environment variables set by the hosting client.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentKind {
    ClaudeCode,
    Copilot,
    Cursor,
    Cline,
    Aider,
    /// Any client whose name was not recognised.
    Unknown(String),
}

impl AgentKind {
    /// Derive the kind from a raw client name string (case-insensitive).
    #[must_use]
    pub fn from_client_name(name: &str) -> Self {
        let lower = name.to_lowercase();
        match lower.as_str() {
            s if s.contains("claude") => Self::ClaudeCode,
            s if s.contains("copilot") => Self::Copilot,
            s if s.contains("cursor") => Self::Cursor,
            s if s.contains("cline") => Self::Cline,
            s if s.contains("aider") => Self::Aider,
            _ => Self::Unknown(name.to_owned()),
        }
    }

    /// A stable, lowercase string identifier suitable for log fields and metrics.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::ClaudeCode    => "claude-code",
            Self::Copilot       => "copilot",
            Self::Cursor        => "cursor",
            Self::Cline         => "cline",
            Self::Aider         => "aider",
            Self::Unknown(name) => name.as_str(),
        }
    }
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Metadata about the MCP client connected to this server session.
#[derive(Debug, Clone)]
pub struct AgentClient {
    /// Raw `clientInfo.name` from the MCP `initialize` request.
    pub name: String,
    /// Raw `clientInfo.version` from the MCP `initialize` request.
    pub version: String,
}

impl AgentClient {
    /// Derive the [`AgentKind`] from this client's name.
    #[must_use]
    pub fn kind(&self) -> AgentKind {
        AgentKind::from_client_name(&self.name)
    }
}
```

### 1.5.2 — Detection logic

```rust
// unblock-core/src/detection.rs

/// Detects the connected AI client from available signals.
///
/// Priority: MCP `clientInfo` → environment variables → [`AgentKind::Unknown`].
pub struct ClientDetector;

impl ClientDetector {
    /// Attempt to detect the client from environment variables.
    ///
    /// This is a fallback for clients that do not populate `clientInfo` in the
    /// MCP `initialize` request, or for non-MCP invocations.
    ///
    /// Note: `VSCODE_PID` is intentionally excluded — it is set for any VS Code
    /// session, not specifically GitHub Copilot. See design decision D6.
    #[must_use]
    pub fn from_env() -> Option<AgentKind> {
        // Claude Code sets this when launching sub-processes
        if std::env::var("CLAUDE_CODE_ENTRYPOINT").is_ok() {
            return Some(AgentKind::ClaudeCode);
        }
        // GitHub Copilot sets this token when active
        if std::env::var("GITHUB_COPILOT_TOKEN").is_ok() {
            return Some(AgentKind::Copilot);
        }
        // Cursor sets a trace ID for its internal telemetry
        if std::env::var("CURSOR_TRACE_ID").is_ok() {
            return Some(AgentKind::Cursor);
        }
        None
    }

    /// Resolve the client kind using the best available signal.
    ///
    /// MCP `clientInfo` takes precedence over environment variables.
    /// Falls back to `AgentKind::Unknown("unknown")` if no signal is present.
    #[must_use]
    pub fn resolve(mcp_client: Option<&AgentClient>) -> AgentKind {
        mcp_client
            .map(|c| c.kind())
            .or_else(|| Self::from_env())
            .unwrap_or_else(|| AgentKind::Unknown("unknown".into()))
    }
}
```

### 1.5.3 — Server state and `initialize` handler (Approach B: OnceLock)

rmcp's default `initialize` handler stores `InitializeRequestParams` in `Peer<RoleServer>`
via `set_peer_info()`. We override it to additionally resolve and cache `AgentKind`.

```rust
// unblock-mcp/src/server.rs (additions)

use std::sync::OnceLock;
use unblock_core::client::{AgentClient, AgentKind};
use unblock_core::detection::ClientDetector;

pub struct ServerState {
    pub config: Arc<Config>,
    pub client: Arc<GitHubClient>,
    pub cache:  Arc<GraphCache>,
    /// Resolved once during MCP `initialize` handshake.
    /// `OnceLock` guarantees single write, lock-free reads.
    pub agent_kind: OnceLock<AgentKind>,
}

impl ServerHandler for UnblockServer {
    fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<InitializeResult, McpError>> + Send + '_ {
        // Build AgentClient from MCP clientInfo
        let agent_client = AgentClient {
            name:    request.client_info.name.clone(),
            version: request.client_info.version.clone(),
        };

        // Resolve kind once and store in OnceLock
        let kind = ClientDetector::resolve(Some(&agent_client));
        let _ = self.state.agent_kind.set(kind.clone());

        tracing::info!(
            client.name    = &agent_client.name,
            client.version = &agent_client.version,
            client.kind    = %kind,
            "mcp client connected"
        );

        // Delegate to rmcp default for peer_info storage
        if context.peer.peer_info().is_none() {
            context.peer.set_peer_info(request);
        }

        std::future::ready(Ok(self.get_info()))
    }
}
```

### 1.5.4 — `SessionMeta` in `prime`

```rust
// unblock-mcp/src/tools/prime.rs (additions)

#[derive(Debug, Serialize)]
pub struct SessionMeta {
    /// Raw client name from MCP initialize (e.g., "claude-code", "github.copilot").
    pub agent_client: String,
    /// Normalised kind string (e.g., "claude-code", "copilot", "unknown").
    pub agent_kind: String,
    /// Value of the `Agent` Projects V2 field for this session, if configured.
    pub agent_field: Option<String>,
    /// UTC timestamp when the MCP session was initialised.
    pub connected_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct PrimeResult {
    pub in_progress: Vec<IssueSummary>,
    pub ready:       Vec<IssueSummary>,
    pub blocked:     Vec<IssueSummary>,
    pub hotspots:    Vec<IssueSummary>,
    pub stale:       Vec<IssueSummary>,
    pub session:     SessionMeta,     // ← new field
}
```

### 1.5.5 — Span fields (all tool handlers)

Tool handlers read `AgentKind` from `ServerState.agent_kind` (lock-free via `OnceLock`).
Raw client name is available from `Peer<RoleServer>.peer_info()` if needed, but for span
fields the kind string is sufficient.

```rust
// Pattern applied to every tool handler
async fn handle_ready(&self, params: ReadyParams) -> Result<ReadyResult> {
    let kind = self.state.agent_kind.get()
        .map(|k| k.as_str().to_owned())
        .unwrap_or_else(|| "unknown".into());

    let _span = tracing::info_span!(
        "tool.ready",
        agent.kind = %kind,
        // ... other tool-specific fields
    ).entered();

    // handler body unchanged
}
```

Since `OnceLock::get()` is lock-free, no helper function is needed — the pattern above is
cheap enough to inline in each handler (one `.get()` call + one `as_str()`).

---

## Test Plan

| Test ID | Kind | Scope | Assertion |
|---|---|---|---|
| `client_kind_from_known_names` | Unit | `AgentKind::from_client_name` | Table-driven: "claude-code", "Claude Code", "CLAUDE" → `ClaudeCode`; "github.copilot" → `Copilot`; etc. |
| `client_kind_unknown_passthrough` | Unit | `AgentKind::from_client_name` | Arbitrary string → `Unknown(string)` |
| `client_kind_display_roundtrip` | Unit | `AgentKind::as_str` + `Display` | Known variants: `from_client_name(kind.as_str()) == kind` |
| `detector_env_claude` | Unit | `ClientDetector::from_env` | Set `CLAUDE_CODE_ENTRYPOINT=1` → `Some(ClaudeCode)` |
| `detector_env_copilot_token` | Unit | `ClientDetector::from_env` | Set `GITHUB_COPILOT_TOKEN=ghu_xxx` → `Some(Copilot)` |
| `detector_env_cursor` | Unit | `ClientDetector::from_env` | Set `CURSOR_TRACE_ID=abc` → `Some(Cursor)` |
| `detector_env_vscode_pid_ignored` | Unit | `ClientDetector::from_env` | Set only `VSCODE_PID=1234` (no `GITHUB_COPILOT_TOKEN`) → `None` (not Copilot). See D6 |
| `detector_env_none` | Unit | `ClientDetector::from_env` | No env vars set → `None` |
| `detector_resolve_mcp_overrides_env` | Unit | `ClientDetector::resolve` | `mcp_client = Some(AgentClient { name: "cursor", .. })` + `GITHUB_COPILOT_TOKEN` set → `Cursor` (MCP wins) |
| `detector_resolve_unknown_fallback` | Unit | `ClientDetector::resolve` | No MCP client + no env vars → `Unknown("unknown")` |
| `initialize_stores_agent_kind` | Integration | `UnblockServer::initialize` | Mock `InitializeRequestParams` with `client_info.name = "claude-code"` → `ServerState.agent_kind` contains `AgentKind::ClaudeCode` via `OnceLock` |
| `initialize_env_fallback` | Integration | `UnblockServer::initialize` | No `clientInfo` name match, `CURSOR_TRACE_ID` set → `AgentKind::Cursor` stored |
| `prime_includes_session_meta` | Integration | `prime` tool | Call `prime` after initialize → response JSON has `session.agent_client`, `session.agent_kind`, `session.connected_at` fields |
| `span_fields_present` | Integration | all tool handlers | Capture tracing output. Call `ready`. Assert JSON log contains `agent.client` and `agent.kind` fields |
| `fuzz_from_client_name` | Property | `AgentKind::from_client_name` | `proptest`: arbitrary `String` input → never panics, always returns a valid `AgentKind` |

---

## Epic Dependency Graph (updated)

```
Phase 1
  1.1 Workspace ──► 1.2 Core ──► 1.3 GitHub API ──► 1.4 MCP Tools (includes prime as 1.4.14)
                                                      ├──► 1.5 Detection  ◄── 1.4.14 (prime)
                                                      └──► 1.6 Reconciliation ◄── 1.4.14 (prime)

Phase 2
  2.1 Tools (remaining) ◄── 1.4
  2.2 Plugin     ◄── 2.1
  2.3 Docs       ◄── 2.1
```

`1.5` and `1.6` are leaf epics in Phase 1 — no Phase 2 work depends on them. They can be
developed in parallel after `1.4.14` (prime) is complete.

---

## Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Client doesn't populate `clientInfo` | Low — detection silently falls back to env/Unknown | Two-level fallback. `Unknown` is a valid, non-fatal state. Server always starts |
| `clientInfo.name` format changes across client versions | Low — substring matching is tolerant | `from_client_name` uses `contains()` not exact match. Covered by fuzz test |
| New AI clients not in the enum | None — `Unknown(String)` captures them | Log the raw name. Enum extended via a trivial PR when a new client is validated |
| `OnceLock` never set (initialize not called) | Low — tool handlers fall back to `"unknown"` | `OnceLock::get()` returns `None`, handled by `.unwrap_or_else()` |

---

## Effort Estimate

| Task | Estimated time |
|---|---|
| 1.5.1 Types | 1–2 hours |
| 1.5.2 Detector | 1 hour |
| 1.5.3 MCP capture (`OnceLock` + `initialize` override) | 1.5 hours |
| 1.5.4 `prime` enrichment | 2 hours |
| 1.5.5 Span fields | 2 hours |
| 1.5.6 Tests | 3 hours |
| **Total** | **~1.5 focused days** |

---

## Updated Task Summary (Phase 1)

| Phase | Epics | Tasks | Focused days |
|---|---|---|---|
| Phase 1 — previous | 4 | 34 | ~20 |
| Epic 1.5 (this) | 1 | 6 | ~1.5 |
| Epic 1.6 (reconciliation) | 1 | ~6 | ~1.75 |
| Task 1.4.14 (prime) | — | 1 | ~1 |
| **Phase 1 — revised** | **6** | **~47** | **~24.25** |
