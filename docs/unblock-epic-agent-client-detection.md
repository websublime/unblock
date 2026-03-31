# Unblock — Epic 2.4: Agent Client Detection

**Dependency-aware task tracking for AI agents, powered by GitHub.**

| | |
|---|---|
| **Epic** | 2.4 — Agent Client Detection |
| **Version target** | v0.2.0 |
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
| Domain types (`AgentKind`, `AgentClient`) | `unblock-core` | `types::client` |
| Detection logic (`ClientDetector`) | `unblock-core` | `detection` |
| MCP `initialize` capture | `unblock-mcp` | `server` |
| `SessionMeta` in `prime` output | `unblock-mcp` | `tools::prime` |
| Span fields | `unblock-mcp` | all tool handlers |

This follows the existing layering: `unblock-core` owns domain types and pure logic;
`unblock-mcp` owns runtime behaviour, I/O, and MCP protocol integration.

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

---

## Tasks

### Epic 2.4 — Agent Client Detection

**Goal:** Identify which AI client is connected and surface this in logs, `prime` output, and
tracing spans — without affecting tool behaviour.

**Depends on:** Epic 1.4 (MCP tools foundation), Epic 2.1.1 (`prime` tool)

| Task | Description | Definition of Done | Ref |
|---|---|---|---|
| **2.4.1** `AgentKind` + `AgentClient` types | In `unblock-core/src/types/client.rs`. `AgentKind` enum: `ClaudeCode`, `Copilot`, `Cursor`, `Cline`, `Aider`, `Unknown(String)`. `AgentClient { name: String, version: String }`. `AgentKind::from_client_name(&str)` — case-insensitive substring match. `AgentKind::as_str() -> &str`. `impl Display for AgentKind`. All types derive `Debug, Clone, PartialEq` | Compiles. Unit tests: known names → correct variant. Unrecognised name → `Unknown(name)`. `Display` emits same string as `as_str()`. `cargo doc` clean | ARCH §5 |
| **2.4.2** `ClientDetector` | In `unblock-core/src/detection.rs`. `ClientDetector::from_env() -> Option<AgentKind>` — reads `CLAUDE_CODE_ENTRYPOINT`, `VSCODE_PID`, `GITHUB_COPILOT_TOKEN`, `CURSOR_TRACE_ID`. `ClientDetector::resolve(mcp_client: Option<&AgentClient>) -> AgentKind` — MCP → env → `Unknown`. Both methods `#[must_use]` | Unit tests: each known env var → correct kind. MCP overrides env. Both absent → `Unknown`. No panics, no I/O side effects. Pure function, no `async` | — |
| **2.4.3** MCP `initialize` capture | In `unblock-mcp/src/server.rs`. Add `client: Arc<RwLock<Option<AgentClient>>>` field to `UnblockServer`. In the `initialize` handler: extract `params.client_info`, construct `AgentClient`, store via `ClientDetector::resolve`, emit `tracing::info!` event with `client.name`, `client.version`, `client.kind` fields | Integration test: mock `initialize` with known `clientInfo` → `AgentKind::ClaudeCode` stored. Second call: missing `clientInfo` → env fallback path exercised. `tracing` event present in captured output | ARCH §9 |
| **2.4.4** `SessionMeta` in `prime` output | In `unblock-mcp/src/tools/prime.rs`. Define `SessionMeta { agent_client: String, agent_kind: String, agent_field: Option<String>, connected_at: DateTime<Utc> }`. Add `session: SessionMeta` to `PrimeResult`. Populate from server state: `AgentClient.name` → `agent_client`, `AgentKind.as_str()` → `agent_kind`, `Config.agent` → `agent_field` | Integration test: `prime` response JSON includes `session` object. Fields populated correctly for known client. `agent_field` is `None` when `UNBLOCK_AGENT` not set | PRD §6.3, ARCH §10 |
| **2.4.5** `agent.client` span fields | In every tool handler in `unblock-mcp/src/tools/`. Add `agent.client` and `agent.kind` as fields on the root `tracing::info_span!` for each tool call. Pull from shared server state | JSON log output includes `agent.client` and `agent.kind` fields on all tool spans. Verified by a log-capture integration test that calls `ready` and asserts field presence. Token not leaked | ARCH §14.1 |
| **2.4.6** Tests | Unit: all `AgentKind` variants (table-driven). `ClientDetector` env + MCP paths. `from_client_name` fuzz: random strings never panic. Integration: full `initialize → prime` flow with mock client | >80% coverage on `types::client` and `detection` modules. All tests green. `cargo clippy -D warnings` clean | Quality Gate |

---

## Implementation Notes

### 2.4.1 — Type signatures (idiomatic Rust)

```rust
// unblock-core/src/types/client.rs

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

### 2.4.2 — Detection logic

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
    #[must_use]
    pub fn from_env() -> Option<AgentKind> {
        // Claude Code sets this when launching sub-processes
        if std::env::var("CLAUDE_CODE_ENTRYPOINT").is_ok() {
            return Some(AgentKind::ClaudeCode);
        }
        // GitHub Copilot runs inside VS Code; VSCODE_PID is always set
        if std::env::var("VSCODE_PID").is_ok()
            || std::env::var("GITHUB_COPILOT_TOKEN").is_ok()
        {
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

### 2.4.3 — Server state and `initialize` handler

```rust
// unblock-mcp/src/server.rs (additions)

pub struct UnblockServer {
    github: Arc<GitHubClient>,
    graph:  Arc<RwLock<DependencyGraph>>,
    config: Arc<Config>,
    // Populated during the MCP initialize handshake.
    // None until the first initialize call is processed.
    client: Arc<RwLock<Option<AgentClient>>>,
}

impl ServerHandler for UnblockServer {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let agent_client = params.client_info.map(|info| AgentClient {
            name:    info.name,
            version: info.version,
        });

        let kind = ClientDetector::resolve(agent_client.as_ref());

        tracing::info!(
            client.name    = agent_client.as_ref().map(|c| c.name.as_str()).unwrap_or("unknown"),
            client.version = agent_client.as_ref().map(|c| c.version.as_str()).unwrap_or("unknown"),
            client.kind    = %kind,
            "mcp client connected"
        );

        *self.client.write().await = agent_client;

        Ok(InitializeResult {
            server_info: ServerInfo {
                name:    "unblock".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            ..Default::default()
        })
    }
}
```

### 2.4.4 — `SessionMeta` in `prime`

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

### 2.4.5 — Span fields (all tool handlers)

```rust
// Pattern applied to every tool handler
async fn handle_ready(&self, params: ReadyParams) -> Result<ReadyResult> {
    let client = self.client.read().await;
    let kind   = ClientDetector::resolve(client.as_ref());

    let _span = tracing::info_span!(
        "tool.ready",
        agent.client = client.as_ref().map(|c| c.name.as_str()).unwrap_or("unknown"),
        agent.kind   = %kind,
        // ... other fields
    ).entered();

    // handler body unchanged
}
```

To avoid repeating this in every handler, extract a helper:

```rust
// unblock-mcp/src/server.rs

impl UnblockServer {
    /// Returns `(client_name, kind_str)` for use in tracing spans.
    pub async fn client_span_fields(&self) -> (String, String) {
        let guard = self.client.read().await;
        let name  = guard.as_ref().map(|c| c.name.clone())
                        .unwrap_or_else(|| "unknown".into());
        let kind  = ClientDetector::resolve(guard.as_ref()).to_string();
        (name, kind)
    }
}
```

---

## Test Plan

| Test ID | Kind | Scope | Assertion |
|---|---|---|---|
| `client_kind_from_known_names` | Unit | `AgentKind::from_client_name` | Table-driven: "claude-code", "Claude Code", "CLAUDE" → `ClaudeCode`; "github.copilot" → `Copilot`; etc. |
| `client_kind_unknown_passthrough` | Unit | `AgentKind::from_client_name` | Arbitrary string → `Unknown(string)` |
| `client_kind_display_roundtrip` | Unit | `AgentKind::as_str` + `Display` | Known variants: `from_client_name(kind.as_str()) == kind` |
| `detector_env_claude` | Unit | `ClientDetector::from_env` | Set `CLAUDE_CODE_ENTRYPOINT=1` → `Some(ClaudeCode)` |
| `detector_env_copilot_vscode` | Unit | `ClientDetector::from_env` | Set `VSCODE_PID=1234` → `Some(Copilot)` |
| `detector_env_cursor` | Unit | `ClientDetector::from_env` | Set `CURSOR_TRACE_ID=abc` → `Some(Cursor)` |
| `detector_env_none` | Unit | `ClientDetector::from_env` | No env vars set → `None` |
| `detector_resolve_mcp_overrides_env` | Unit | `ClientDetector::resolve` | `mcp_client = Some(AgentClient { name: "cursor", .. })` + `VSCODE_PID` set → `Cursor` (from MCP, not Copilot from env) |
| `detector_resolve_unknown_fallback` | Unit | `ClientDetector::resolve` | No MCP client + no env vars → `Unknown("unknown")` |
| `initialize_stores_client` | Integration | `UnblockServer::initialize` | Mock `InitializeParams` with `client_info = Some({ name: "claude-code", version: "1.0" })` → server state contains `AgentClient { name: "claude-code", .. }` |
| `initialize_missing_client_info` | Integration | `UnblockServer::initialize` | `client_info = None`, no env vars → server state `None`, kind resolves to `Unknown` |
| `prime_includes_session_meta` | Integration | `prime` tool | Call `prime` after initialize → response JSON has `session.agent_client`, `session.agent_kind`, `session.connected_at` fields |
| `span_fields_present` | Integration | all tool handlers | Capture tracing output. Call `ready`. Assert JSON log contains `agent.client` and `agent.kind` fields |
| `fuzz_from_client_name` | Property | `AgentKind::from_client_name` | `proptest`: arbitrary `String` input → never panics, always returns a valid `AgentKind` |

---

## Epic Dependency Graph (updated)

```
Phase 2
  2.1 Tools      ◄── 1.4
  2.2 Plugin     ◄── 2.1
  2.3 Docs       ◄── 2.1
  2.4 Detection  ◄── 1.4 (server state) + 2.1.1 (prime tool)
```

`2.4` has no dependents in Phase 2. It is a leaf enhancement that can be developed in
parallel with `2.2` and `2.3` once `1.4` and `2.1.1` are complete.

---

## Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Client doesn't populate `clientInfo` | Low — detection silently falls back to env/Unknown | Two-level fallback. `Unknown` is a valid, non-fatal state. Server always starts |
| `clientInfo.name` format changes across client versions | Low — substring matching is tolerant | `from_client_name` uses `contains()` not exact match. Covered by fuzz test |
| New AI clients not in the enum | None — `Unknown(String)` captures them | Log the raw name. Enum extended via a trivial PR when a new client is validated |
| RwLock contention on `client` field | None — `initialize` is called once; all reads are shared | Single writer (initialize), many readers (tool handlers). `RwLock` is the right primitive |

---

## Effort Estimate

| Task | Estimated time |
|---|---|
| 2.4.1 Types | 1–2 hours |
| 2.4.2 Detector | 1 hour |
| 2.4.3 MCP capture | 2 hours |
| 2.4.4 `prime` enrichment | 2 hours |
| 2.4.5 Span fields | 2 hours |
| 2.4.6 Tests | 3 hours |
| **Total** | **~1.5 focused days** |

---

## Updated Task Summary (Phase 2)

| Phase | Epics | Tasks | Focused days |
|---|---|---|---|
| Phase 2 — previous | 3 | 14 | ~8 |
| Epic 2.4 (this) | 1 | 6 | ~1.5 |
| **Phase 2 — revised** | **4** | **20** | **~9.5** |
