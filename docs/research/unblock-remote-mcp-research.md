# Unblock — Remote MCP Requirements Research

**Exploring the remote MCP path: shared tools crate, Streamable HTTP transport, axum.**

| | |
|---|---|
| **Date** | April 2026 |
| **Status** | Research — not committed |
| **Author** | Miguel Ramos |
| **Org** | websublime |
| **Depends on** | `unblock-architecture-github.md` v1.2.0-draft |

---

## 1. Motivation

The current `unblock-mcp` binary uses stdio transport — a deliberate architectural choice that
delivers zero infrastructure, zero config, and zero network exposure. That choice remains correct
for the local use case and will not change.

This document explores what a **parallel remote MCP binary** would look like if it were to exist
alongside the local binary, sharing the maximum possible code. The goals are:

- Identify the exact crate boundary where tools should live so both binaries reuse them
- Define the transport choice (Streamable HTTP) and justify it
- Define the auth model for remote connections
- Define the shared graph cache model across sessions
- Define the webhook strategy for real-time cache invalidation
- Define the GHE strategy for self-hosted deployments
- Produce a workspace plan with zero changes to `unblock-core` and `unblock-github`

---

## 2. The Core Problem with the Current Structure

Today, the 18 tool implementations live inside `unblock-mcp`:

```
unblock-mcp/src/
  server.rs      ← MCP bootstrap + tool registration
  tools/
    ready.rs
    claim.rs
    close.rs
    ... (15 more)
```

A second binary (`unblock-mcp-remote`) that needs the same tools has two options:

- **Duplicate** — copy the tool implementations. Unacceptable: two sources of truth.
- **Depend on `unblock-mcp` as a lib** — possible, but awkward: a binary-centric crate
  being used as a library. Semantics are wrong.

The clean solution is a dedicated shared crate.

---

## 3. New Crate: `unblock-tools`

`unblock-tools` is a **library crate** that owns all 18 tool implementations. It has no binary,
no MCP bootstrap, no transport. It is pure tool logic: validate input, call GitHub, rebuild
graph, return result.

```
crates/
  unblock-core/          ← zero changes
  unblock-github/        ← zero changes
  unblock-tools/         ← NEW: shared tool implementations
  unblock-mcp/           ← becomes thin binary (stdio bootstrap only)
  unblock-mcp-remote/    ← NEW: thin binary (HTTP bootstrap only)
  unblock-app/           ← zero changes
```

### 3.1 What `unblock-tools` owns

All tool execution functions, their input/output types, and the `ServerState` struct that both
binaries share:

```rust
// crates/unblock-tools/src/lib.rs

pub mod state;     // ServerState: shared config, GitHub client, cache
pub mod tools;     // 18 tool modules
pub mod errors;    // McpError conversions

// Re-exports for convenience
pub use state::ServerState;
```

```rust
// crates/unblock-tools/src/state.rs

pub struct ServerState {
    pub config:  Config,
    pub github:  GitHubClient,
    pub cache:   GraphCache,
}
```

```rust
// crates/unblock-tools/src/tools/ready.rs

pub async fn execute(
    state: &ServerState,
    params: ReadyParams,
) -> Result<ReadyResult, McpError> {
    // exactly as today — zero changes to the logic
}
```

### 3.2 `unblock-tools/Cargo.toml`

```toml
[package]
name    = "unblock-tools"
version = "0.1.0"
edition = "2024"

[lib]
name = "unblock_tools"
path = "src/lib.rs"

[dependencies]
unblock-core   = { path = "../unblock-core" }
unblock-github = { path = "../unblock-github" }
rmcp           = { workspace = true, features = ["server"] }
schemars       = { workspace = true }
serde          = { workspace = true }
serde_json     = { workspace = true }
tokio          = { workspace = true }
tracing        = { workspace = true }
snafu          = { workspace = true }
```

No transport features. No HTTP. No axum. Pure tool logic only.

### 3.3 What `unblock-mcp` becomes

A thin bootstrap — ~50 lines:

```rust
// crates/unblock-mcp/src/main.rs

use rmcp::{ServiceExt, transport::stdio};
use unblock_tools::state::ServerState;

mod server; // UnblockServer: tool registration against unblock-tools functions

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    init_tracing_stderr(); // JSON to stderr; stdio is MCP protocol
    let state = ServerState::bootstrap(config).await?;
    let server = UnblockServer::new(state);
    server.serve(stdio()).await?;
    Ok(())
}
```

```rust
// crates/unblock-mcp/src/server.rs

use unblock_tools::{ServerState, tools};

pub struct UnblockServer {
    state: Arc<ServerState>,
}

#[tool(tool_box)]
impl UnblockServer {
    #[tool(name = "ready", description = "...")]
    async fn ready(&self, params: ReadyParams) -> Result<ReadyResult, McpError> {
        tools::ready::execute(&self.state, params).await
    }
    // ... 17 more delegations — no logic here
}
```

The `unblock-mcp` crate is now a **pure bootstrap**: transport wiring + tool registration.
All logic remains in `unblock-tools`.

---

## 4. Transport: Streamable HTTP Only

The MCP specification defines three transports:

| Transport | MCP spec status | Direction |
|---|---|---|
| stdio | Stable | Local process only |
| SSE (Server-Sent Events) | Deprecated (spec 2025-03) | Server push over HTTP GET |
| Streamable HTTP | Current standard (spec 2025-03) | Single endpoint, bidirectional |

`unblock-mcp-remote` implements **Streamable HTTP only**. SSE is excluded deliberately:

- SSE is deprecated in the current MCP spec — new clients implement Streamable HTTP
- SSE requires two endpoints (`GET /sse`, `POST /message`) with session correlation complexity
- Streamable HTTP is a single endpoint (`POST /mcp`) with optional streaming response
- `rmcp`'s `transport-streamable-http-server` feature covers it

The single endpoint contract:

```
POST /mcp
Authorization: Bearer <github_token>

Request body:  JSON-RPC message (client → server)
Response body: JSON-RPC message or streaming JSON-RPC messages (server → client)
```

Long-running tool calls (e.g., `setup --migrate` over a large repo) use chunked transfer
encoding to stream progress. Short tool calls return a single JSON response.

### 4.1 rmcp feature flags

```toml
# unblock-mcp (stdio — unchanged)
rmcp = { workspace = true, features = ["server", "transport-io"] }

# unblock-mcp-remote (Streamable HTTP)
rmcp = { workspace = true, features = ["server", "transport-streamable-http-server"] }
```

### 4.2 Why axum

The `rmcp` Streamable HTTP transport exposes an axum `Router` directly. The server
integration is composition of routers — no adapters, no glue code. Choosing any other
framework would require bridging to that axum `Router`, introducing unnecessary indirection.

axum additionally provides:

- `State<T>` extractor — ergonomic shared state (the `SharedGraphCache`)
- `FromRequestParts` — clean GitHub token extraction pattern
- Tower middleware — auth, tracing, and future rate limiting as independent layers
- Same tokio runtime as `unblock-github` and `unblock-core` — zero runtime conflicts

---

## 5. Auth Model

### 5.1 GitHub Token as Identity

The remote server has no user database, no API keys, no sessions to store. The
`GITHUB_TOKEN` the developer already possesses is simultaneously:

- **Identity** — which GitHub user is calling
- **Credential** — what repos are accessible
- **Scope** — what operations are permitted

Every request carries the token in the standard Authorization header:

```
Authorization: Bearer ghp_xxxxxxxxxxxxxxxxxxxx
```

### 5.2 Token Extractor

```rust
// crates/unblock-mcp-remote/src/auth.rs

use secrecy::SecretString;

/// Extracted from Authorization header on every request.
/// Never logged, never stored beyond request lifetime.
#[derive(Clone)]
pub struct GitHubToken(pub SecretString);

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for GitHubToken {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|t| GitHubToken(SecretString::new(t.to_string().into())))
            .ok_or(AppError::Unauthorized)
    }
}
```

### 5.3 Token Validation

Token validity is verified once per session during the MCP `initialize` handshake via a
`GET /user` call to the GitHub API. The result is cached per token fingerprint for 5 minutes:

```rust
// crates/unblock-mcp-remote/src/auth.rs

pub struct IdentityCache {
    inner: DashMap<TokenFingerprint, (GitHubIdentity, Instant)>,
    ttl:   Duration, // 300s
}

impl IdentityCache {
    pub async fn resolve(
        &self,
        token: &GitHubToken,
        client: &reqwest::Client,
    ) -> Result<GitHubIdentity, AppError> {
        let fingerprint = TokenFingerprint::of(token); // SHA-256, never the token itself

        if let Some(entry) = self.inner.get(&fingerprint) {
            if entry.1.elapsed() < self.ttl {
                return Ok(entry.0.clone());
            }
        }

        let identity = fetch_github_identity(token, client).await?;
        self.inner.insert(fingerprint, (identity.clone(), Instant::now()));
        Ok(identity)
    }
}
```

One GitHub API call per session start. All subsequent tool calls within the session use
the cached identity. Zero overhead per tool invocation.

---

## 6. Shared Graph Cache

### 6.1 Problem

In the local binary, the graph cache is process-scoped — it lives and dies with the process.
Each Claude Code session spawns a new process, a new cache, a cold start.

In the remote binary, the process is always running. Multiple agents and sessions can
connect to the same repo. The cache should survive between sessions — **same repo, same
graph, no cold start after the first session**.

### 6.2 Cache Key

The cache is keyed by `(owner/repo, token_fingerprint)`. The token fingerprint (SHA-256 of
the token) ensures that two users connecting to the same repo get independent caches —
their GitHub tokens have different permissions and may see different issues.

```rust
#[derive(Hash, Eq, PartialEq, Clone)]
pub struct CacheKey {
    pub repo:               RepoKey,            // "owner/repo"
    pub token_fingerprint:  TokenFingerprint,   // SHA-256(token)
}
```

### 6.3 Implementation

```rust
// crates/unblock-mcp-remote/src/cache.rs

pub struct SharedGraphCache {
    inner: DashMap<CacheKey, Arc<RwLock<CacheEntry>>>,
}

struct CacheEntry {
    graph:       DependencyGraph,
    computed_at: Instant,
    ttl:         Duration,   // from UNBLOCK_CACHE_TTL, same as local
    stale:       bool,
}

impl SharedGraphCache {
    /// Get or build. If the entry is expired, rebuild and update.
    /// Concurrent requests for the same key wait on the same RwLock — no thundering herd.
    pub async fn get_or_build(
        &self,
        key:     CacheKey,
        builder: impl Future<Output = Result<DependencyGraph>>,
    ) -> Result<Arc<RwLock<CacheEntry>>> {
        if let Some(entry) = self.inner.get(&key) {
            if !entry.read().await.is_expired() {
                return Ok(Arc::clone(&*entry));
            }
        }

        let graph = builder.await?;
        let entry = Arc::new(RwLock::new(CacheEntry::new(graph, DEFAULT_TTL)));
        self.inner.insert(key, Arc::clone(&entry));
        Ok(entry)
    }

    /// Called by webhook handler. Drops the entry — next tool call rebuilds.
    pub fn invalidate_repo(&self, repo: &RepoKey) {
        self.inner.retain(|k, _| &k.repo != repo);
    }
}
```

The `DashMap` provides concurrent access without a global lock. Each `CacheEntry` is behind
its own `RwLock`, allowing concurrent reads and exclusive writes. This matches the
concurrency model already established in the local binary — no new patterns required.

---

## 7. Session Context via `initialize`

In the local binary, the repo is auto-detected from `cwd` (git remote). In the remote
binary, the `cwd` is the server's working directory — useless for per-session context.

The MCP `initialize` handshake provides the correct hook. The client declares the repo in
the `meta` field:

```json
{
  "method": "initialize",
  "params": {
    "clientInfo": { "name": "claude-code", "version": "1.2.3" },
    "meta": {
      "unblock:repo":    "websublime/unblock",
      "unblock:project": "42"
    }
  }
}
```

The server resolves this in the `initialize` handler and stores it in session state. If
`unblock:repo` is absent, the server attempts auto-detection via the GitHub API (list repos
accessible by the token and infer from context) or returns a `ProjectNotConfigured` error
with guidance.

A cache warm-up is dispatched immediately after `initialize` without blocking the response:

```rust
async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult, McpError> {
    let repo = resolve_repo_from_params(&params, &self.identity).await?;
    self.session.set_repo(repo.clone());

    // Warm up cache in background — agent gets initialize response immediately
    let session = self.session.clone();
    tokio::spawn(async move {
        let _ = session.warm_cache().await;
    });

    Ok(build_initialize_result())
}
```

---

## 8. Webhook Handler

The local binary relies on TTL-based invalidation (default 30s). When a human closes an
issue in the GitHub UI, the local binary sees the change at most 30s late.

The remote binary can receive GitHub webhooks and invalidate the cache immediately:

```rust
// crates/unblock-mcp-remote/src/webhooks.rs

pub async fn github_webhook(
    State(state): State<AppState>,
    headers:      HeaderMap,
    body:         Bytes,
) -> Result<StatusCode, AppError> {
    verify_hmac_sha256(&headers, &body, &state.webhook_secret)?;

    let event: GitHubWebhookEvent = serde_json::from_slice(&body)?;

    match &event {
        GitHubWebhookEvent::Issues(e) => {
            match e.action {
                IssueAction::Closed
                | IssueAction::Reopened
                | IssueAction::Opened
                | IssueAction::Labeled
                | IssueAction::Unlabeled => {
                    let repo = RepoKey::from(&e.repository);
                    state.graph_cache.invalidate_repo(&repo);
                    tracing::info!(repo = %repo, action = ?e.action, "Cache invalidated via webhook");
                }
                _ => {}
            }
        }
        _ => {} // ignore PR, push, etc.
    }

    Ok(StatusCode::NO_CONTENT)
}
```

The webhook does not trigger a rebuild — it only invalidates. The next tool call triggers
the rebuild lazily. This keeps the webhook handler fast (<1ms) and avoids redundant rebuilds
when multiple webhook events arrive in quick succession (e.g., cascade closes).

### 8.1 Webhook Configuration

The GitHub App or repository webhook is configured to send `issues` events to:

```
POST https://unblock.example.com/webhooks/github
```

The `WEBHOOK_SECRET` env var is used server-side for HMAC-SHA256 verification. The secret
is set when registering the webhook in GitHub's settings.

---

## 9. GHE Support

GHE support in the remote binary follows the same pattern as the local binary — the
`GITHUB_API_URL` env var already present in `unblock-github`. No new code paths.

The distinction is **where** the config comes from:

| Binary | `GITHUB_API_URL` source |
|---|---|
| `unblock-mcp` (stdio) | Developer's local env, set at process spawn |
| `unblock-mcp-remote` | Server env var, set at deploy time |

For GHE, the binary is self-hosted inside the corporate network:

```bash
docker run \
  -e GITHUB_TOKEN=ghe_xxx \              # service account token (optional default)
  -e GITHUB_API_URL=https://ghe.corp.com/api/v3 \
  -e GITHUB_URL=https://ghe.corp.com \
  -e WEBHOOK_SECRET=xxx \
  -e BIND_ADDR=0.0.0.0:3000 \
  -p 3000:3000 \
  ghcr.io/websublime/unblock-mcp-remote
```

Individual developers still provide their own token via `Authorization: Bearer` — the
`GITHUB_TOKEN` env var on the server is optional (used for webhook-triggered operations
without a user token context). `GITHUB_API_URL` on the server sets the base URL for all
outbound GitHub API calls, routing them to the internal GHE instance.

**No per-request GHE URL override is supported.** Accepting an arbitrary GitHub API URL
per request from an untrusted client introduces SSRF risk. The server routes to one GitHub
instance only — the one configured at deploy time.

---

## 10. Server Bootstrap and Router

```rust
// crates/unblock-mcp-remote/src/main.rs

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = RemoteConfig::from_env()?;
    init_tracing_stdout(&config); // JSON to stdout (no stdio conflict)

    let app_state = AppState {
        graph_cache:    SharedGraphCache::new(),
        identity_cache: IdentityCache::new(Duration::from_secs(300)),
        http_client:    build_http_client(&config),
        webhook_secret: config.webhook_secret.clone(),
        github_api_url: config.github_api_url.clone(),
    };

    // rmcp Streamable HTTP router
    let mcp_router = build_mcp_router(app_state.clone());

    let app = Router::new()
        .nest("/mcp", mcp_router)
        .route("/webhooks/github", post(github_webhook))
        .route("/health", get(|| async { StatusCode::OK }))
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive()),
        )
        .with_state(app_state);

    let listener = TcpListener::bind(&config.bind_addr).await?;
    tracing::info!(addr = %config.bind_addr, "unblock-mcp-remote listening");
    axum::serve(listener, app).await?;
    Ok(())
}
```

```rust
fn build_mcp_router(state: AppState) -> Router<AppState> {
    use rmcp::transport::streamable_http_server::StreamableHttpServer;

    let (server_handle, router) = StreamableHttpServer::new(
        StreamableHttpServerConfig::default(),
        move |session| {
            let state = state.clone();
            async move { UnblockRemoteServer::new(state, session).await }
        },
    );

    // server_handle drives the MCP session lifecycle — spawn it
    tokio::spawn(server_handle.run());

    router
}
```

---

## 11. Workspace Changes

### 11.1 Updated `Cargo.toml`

```toml
[workspace]
members = [
    "crates/unblock-core",          # zero changes
    "crates/unblock-github",        # zero changes
    "crates/unblock-tools",         # NEW: shared tool implementations
    "crates/unblock-mcp",           # becomes thin bootstrap (stdio)
    "crates/unblock-mcp-remote",    # NEW: thin bootstrap (Streamable HTTP)
    "crates/unblock-app",           # zero changes
]
resolver = "2"

[workspace.dependencies]
# existing — unchanged
serde        = { version = "1", features = ["derive"] }
serde_json   = "1"
tokio        = { version = "1", features = ["full"] }
tracing      = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter"] }
reqwest      = { version = "0.12", features = ["json"] }
petgraph     = "0.7"
chrono       = { version = "0.4", features = ["serde"] }
snafu        = "0.8"
anyhow       = "1"
schemars     = "1"
rand         = "0.9"
rmcp         = { version = "1.0", features = ["server"] } # base; transports per crate

# new — remote binary only
axum         = "0.8"
tower        = "0.5"
tower-http   = { version = "0.6", features = ["cors", "trace"] }
dashmap      = "6"
secrecy      = "0.10"
sha2         = "0.10"  # token fingerprint
hmac         = "0.12"  # webhook verification
```

### 11.2 Crate dependency graph (updated)

```
unblock-mcp (bin, stdio)
  └── unblock-tools (lib)
        ├── unblock-github (lib)
        │     └── unblock-core (lib)
        └── unblock-core (lib)

unblock-mcp-remote (bin, Streamable HTTP)
  ├── unblock-tools (lib)       ← same tools, zero duplication
  │     ├── unblock-github
  │     └── unblock-core
  └── axum + tower-http + dashmap + secrecy
```

### 11.3 File structure

```
crates/
├── unblock-core/
│   └── src/
│       ├── types.rs
│       ├── graph.rs
│       ├── cache.rs
│       ├── config.rs
│       └── errors.rs
│
├── unblock-github/
│   └── src/
│       ├── client.rs
│       ├── graphql.rs
│       ├── mutations.rs
│       ├── projects.rs
│       └── errors.rs
│
├── unblock-tools/               ← NEW
│   └── src/
│       ├── lib.rs
│       ├── state.rs             ← ServerState (shared)
│       ├── errors.rs
│       └── tools/
│           ├── mod.rs
│           ├── ready.rs
│           ├── claim.rs
│           ├── close.rs
│           ├── create.rs
│           ├── depends.rs
│           ├── dep_remove.rs
│           ├── show.rs
│           ├── list.rs
│           ├── search.rs
│           ├── update.rs
│           ├── comment.rs
│           ├── blocked.rs
│           ├── prime.rs
│           ├── init.rs
│           ├── setup.rs
│           ├── reopen.rs
│           ├── rework.rs
│           └── stats.rs
│
├── unblock-mcp/                 ← thin stdio bootstrap
│   └── src/
│       ├── main.rs              ← ~50 lines
│       └── server.rs            ← tool registration only
│
├── unblock-mcp-remote/          ← NEW: thin HTTP bootstrap
│   └── src/
│       ├── main.rs
│       ├── server.rs            ← tool registration (same pattern)
│       ├── auth.rs              ← GitHubToken extractor, IdentityCache
│       ├── cache.rs             ← SharedGraphCache (DashMap)
│       ├── config.rs            ← RemoteConfig (env vars)
│       └── webhooks.rs          ← GitHub webhook handler
│
└── unblock-app/                 ← classified, zero changes
```

---

## 12. Environment Variables

### 12.1 Shared (same as local binary)

| Variable | Required | Default | Notes |
|---|---|---|---|
| `GITHUB_API_URL` | No | `https://api.github.com` | Set for GHE: `https://ghe.corp.com/api/v3` |
| `GITHUB_URL` | No | `https://github.com` | Set for GHE: `https://ghe.corp.com` |
| `UNBLOCK_CACHE_TTL` | No | `30` | Seconds. Same TTL semantics as local |
| `UNBLOCK_LOG_LEVEL` | No | `info` | Log level |
| `UNBLOCK_OTEL_ENDPOINT` | No | — | OpenTelemetry collector |

### 12.2 Remote-only

| Variable | Required | Default | Notes |
|---|---|---|---|
| `BIND_ADDR` | No | `0.0.0.0:3000` | TCP bind address |
| `WEBHOOK_SECRET` | No | — | GitHub webhook HMAC-SHA256 secret. If absent, webhook endpoint returns 501 |
| `IDENTITY_CACHE_TTL` | No | `300` | Token identity cache TTL seconds |

No `GITHUB_TOKEN` on the server — each connecting client provides its own token via
`Authorization: Bearer`. A server-level `GITHUB_TOKEN` is only useful for server-initiated
operations (e.g., webhook-triggered rebuilds without a client session). This is optional
and can be omitted for deployments that disable webhook support.

---

## 13. MCP Client Config

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

The config replaces the `command`/`env` block of the local binary. No install required on
the client side. `${GITHUB_TOKEN}` is expanded by the MCP client from the local environment.

For GHE (self-hosted):

```json
{
  "mcpServers": {
    "unblock": {
      "type": "http",
      "url": "https://unblock.internal.corp.com/mcp",
      "headers": {
        "Authorization": "Bearer ${GHE_TOKEN}"
      }
    }
  }
}
```

---

## 14. CI/CD Impact

### 14.1 Separate release tag

The remote binary follows the same independent versioning pattern as `unblock-mcp` vs
`unblock-app`:

```
unblock-mcp-v1.0.0        → cargo-dist → GitHub Releases + Homebrew
unblock-mcp-remote-v1.0.0 → custom workflow → Docker image + GitHub Releases
```

### 14.2 Docker release

```dockerfile
FROM rust:1.82-slim AS builder
WORKDIR /app
COPY . .
RUN cargo build --release -p unblock-mcp-remote

FROM debian:bookworm-slim
RUN apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/unblock-mcp-remote /usr/local/bin/
EXPOSE 3000
CMD ["unblock-mcp-remote"]
```

```yaml
# .github/workflows/remote-release.yml
on:
  push:
    tags: ['unblock-mcp-remote-v*']

jobs:
  docker:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Build and push Docker image
        uses: docker/build-push-action@v5
        with:
          push: true
          tags: ghcr.io/websublime/unblock-mcp-remote:${{ github.ref_name }}
```

### 14.3 GitHub Actions integration

With the remote binary, GitHub Actions review and QA jobs require no binary install:

```yaml
- name: Review task
  uses: anthropics/claude-code-action@v1
  with:
    mcp_config: |
      {
        "unblock": {
          "type": "http",
          "url": "${{ vars.UNBLOCK_REMOTE_URL }}",
          "headers": { "Authorization": "Bearer ${{ secrets.GITHUB_TOKEN }}" }
        }
      }
```

Zero `cargo install`, zero cache warming. The runner connects to the always-warm remote
server and uses the shared graph cache — the graph may already be built from a previous
session on the same repo.

---

## 15. What Does Not Change

This is the most important section. The architectural principle **"GitHub stores, Rust
computes"** is fully preserved in the remote binary. No custom storage is introduced.

| Component | Change |
|---|---|
| `unblock-core` | Zero — graph engine, types, cache struct |
| `unblock-github` | Zero — all GitHub API calls |
| Tool logic | Zero — moved to `unblock-tools`, same code |
| MCP protocol handlers | Zero — rmcp abstracts the transport |
| GitHub as source of truth | Preserved — server stores nothing persistently |
| Stateless across restarts | Preserved — `SharedGraphCache` is reconstructable from GitHub |
| `GITHUB_API_URL` for GHE | Preserved — same env var, same logic |

The only new code is the transport bootstrap (~200 lines), the auth middleware (~60 lines),
the shared cache wrapper (~80 lines), and the webhook handler (~50 lines). Approximately
390 lines of new infrastructure code. All tool logic is zero-change migration from
`unblock-mcp/src/tools/` to `unblock-tools/src/tools/`.

---

## 16. Open Questions

| Question | Notes |
|---|---|
| Should `ServerState` in `unblock-tools` be `Arc`-wrapped internally? | Remote needs `Arc<ServerState>` per session; local uses a single instance. The tools take `&ServerState` today — compatible with both. |
| Webhook endpoint: require authentication? | GitHub sends a shared secret via HMAC — sufficient. No GitHub token auth needed on the webhook endpoint itself. |
| Identity cache eviction strategy? | LRU with max capacity (e.g., 10,000 entries) vs pure TTL. TTL-only is simpler; LRU adds dashmap-lru dependency. Start with TTL. |
| Should `unblock-tools` expose `ServerState::bootstrap()` or should each binary own its bootstrap? | Each binary owns bootstrap — remote needs a different bootstrap path (no cwd detection, no git remote). `unblock-tools` owns only the post-bootstrap state and tools. |
| Rate limit pooling across sessions? | Multiple agents on the same repo share one server process but use different tokens — rate limits are per-token, per GitHub account. No pooling opportunity here. |
