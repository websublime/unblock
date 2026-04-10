# Plan 03 — MCP Production (v1.0.0)

> Phase: 03  
> Version: v1.0.0  
> Crates: `unblock-core`, `unblock-github`, `unblock-mcp`  
> Depends on: Phase 02 (MCP Complete) v0.2.0  
> Required by: Phase 04 (Plugin)  
> Status: not started  
> Companion specs: [01-spec-graph-engine.md](../specs/01-spec-graph-engine.md) · [02-spec-github-client.md](../specs/02-spec-github-client.md)

---

## Table of Contents

1. [Purpose](#1-purpose)
2. [Rust Idioms & Rules](#2-rust-idioms--rules)
3. [Public API Surface](#3-public-api-surface)
4. [Priority & Dependency Legend](#4-priority--dependency-legend)
5. [Epics](#5-epics)
   - [Epic 01 — Cross-Platform Binaries via cargo-dist](#epic-01--cross-platform-binaries-via-cargo-dist)
   - [Epic 02 — Shell & PowerShell Installers](#epic-02--shell--powershell-installers)
   - [Epic 03 — npm Wrapper](#epic-03--npm-wrapper)
   - [Epic 04 — Homebrew Formula](#epic-04--homebrew-formula)
   - [Epic 05 — Materialised Fast Path (Strategy D)](#epic-05--materialised-fast-path-strategy-d)
   - [Epic 06 — GitHub Enterprise Server Support](#epic-06--github-enterprise-server-support)
   - [Epic 07 — GitHub App Authentication](#epic-07--github-app-authentication)
   - [Epic 08 — Coverage & Quality Hardening](#epic-08--coverage--quality-hardening)
   - [Epic 09 — v1.0.0 Release](#epic-09--v100-release)
6. [Definition of Done](#6-definition-of-done)

---

## 1. Purpose

Phase 03 turns the MCP server into a production-grade, installable product. Phase 01 delivered the agent workflow loop. Phase 02 hardened it with resilience, observability, drift detection, and agent awareness. Phase 03 makes it available to everyone.

1. **Distribution.** Cross-platform binaries for 5 targets via `cargo-dist`. Shell and PowerShell installer scripts. An npm wrapper for `npx @unblock/cli`. A Homebrew formula for `brew install websublime/tap/unblock`.

2. **Cold start performance.** The materialised fast path (Strategy D) uses the Ready State Projects V2 field as a persistent cache. On startup, serve the ready queue immediately from field values while rebuilding the graph asynchronously. Reduces cold start from seconds to <500ms for repos with <500 issues.

3. **Enterprise readiness.** GitHub Enterprise Server support via configurable `GITHUB_API_URL` and `GITHUB_URL`. GitHub App authentication for higher rate limits (15,000/hour), org-wide installation, and bot identity.

4. **Quality.** 100% coverage target from this phase onwards. Property tests for all graph invariants. Full integration test suite against live GitHub.

**Phase 03 does not:**
- Add new crates (still 3: core, github, mcp)
- Change the transport (still stdio only — HTTP is Phase 05)
- Add new MCP tools beyond the 20 from Phase 02
- Introduce persistent storage beyond git refs (Strategy D is a persistent cache using GitHub's own Projects V2 fields)

**Outcome:** `v1.0.0` release. Installable on any platform. Production-grade for teams with 500+ issues.

---

## 2. Rust Idioms & Rules

These rules supplement the workspace-wide rules and the Phase 02 rules.

### 2.1 `git2` is vendored

The persistent cache (Strategy D) uses `git2` with `vendored` feature to bundle `libgit2-sys`. This avoids system-level libgit2 dependency which varies across platforms and distributions — critical for cargo-dist cross-compilation.

```toml
[dependencies]
git2 = { version = "0.19", features = ["vendored"] }
```

### 2.2 URL construction is centralised

All GitHub URL construction (REST, GraphQL, raw content) goes through `GitHubClient` methods that respect `api_base_url`. No hardcoded `api.github.com` strings outside of `Config::default()`. GHE Server URL resolution (`/api/v3` → strip for GraphQL) is in one place: `GitHubClient::graphql_url()`.

### 2.3 Authentication is trait-based

`GitHubAuth` trait with two implementations: `TokenAuth` (PAT, existing) and `AppAuth` (GitHub App, new). The `GitHubClient` accepts `Box<dyn GitHubAuth>`. This keeps auth strategy swappable without modifying client internals.

### 2.4 Distribution artefacts are not in the main repo

The Homebrew tap lives in `websublime/homebrew-tap`. The npm package lives in `websublime/unblock-npm`. cargo-dist manages the release workflow in `.github/workflows/release.yml`. These external repos are referenced but not managed by this plan — they are infrastructure.

---

## 3. Public API Surface

### 3.1 Changed files in `unblock-core`

```
unblock-core/src/
  fast_path.rs         ← NEW: FastPathReady, serve_from_fields()
```

### 3.2 Changed files in `unblock-github`

```
unblock-github/src/
  auth.rs              ← NEW: GitHubAuth trait, TokenAuth, AppAuth
  client.rs            ← MODIFIED: accept Box<dyn GitHubAuth>, GHE URL resolution
```

### 3.3 Changed files in `unblock-mcp`

```
unblock-mcp/src/
  main.rs              ← MODIFIED: auth strategy selection at startup
  Cargo.toml           ← MODIFIED: cargo-dist metadata, git2 dependency
```

### 3.4 New external repositories

```
websublime/homebrew-tap/
  Formula/unblock.rb   ← Homebrew formula (auto-updated by cargo-dist)

websublime/unblock-npm/
  package.json         ← npm wrapper package (@unblock/cli)
  bin/cli.js           ← Binary download + execute script
```

---

## 4. Priority & Dependency Legend

### Priority levels

| Level | Meaning |
|---|---|
| **P0** | Absolute blocker — nothing moves forward until this is done |
| **P1** | Critical for the phase to be functional — happy path |
| **P2** | Important but does not block the happy path |
| **P3** | Quality, ergonomics, extra coverage |
| **P4** | Nice to have — included if time permits, does not delay done |

### Dependency fields

- **Priority** — P0 through P4
- **Depends on** — task IDs within this plan
- **Blocked by** — external blockers

---

## 5. Epics

---

### Epic 01 — Cross-Platform Binaries via cargo-dist

**Goal:** Automated release pipeline that produces binaries for 5 target platforms from a single tag push.

**Tools:** `cargo-dist`, GitHub Actions

---

#### Task 01.01 — Initialize cargo-dist configuration

> **Priority:** P0  
> **Depends on:** nothing  
> **Blocked by:** nothing

**File:** `unblock-mcp/Cargo.toml` (dist metadata), `dist-workspace.toml` (workspace root)

Requirements:
- Run `cargo dist init` to generate initial configuration
- Configure 5 release targets:
  - `x86_64-unknown-linux-musl` — static binary, zero system deps
  - `aarch64-unknown-linux-musl` — static binary, zero system deps
  - `x86_64-apple-darwin` — macOS Intel
  - `aarch64-apple-darwin` — macOS Apple Silicon
  - `x86_64-pc-windows-msvc` — Windows
- Configure installers: `shell`, `powershell`
- Tag format: `unblock-mcp-v{version}` (Singular Announcement)
- Linux targets use `musl` for fully static binaries (no glibc dependency)
- The generated `release.yml` workflow handles: plan → build → host → announce

```toml
# crates/unblock-mcp/Cargo.toml
[package.metadata.dist]
installers = ["shell", "powershell"]
targets = [
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
]
```

```toml
# dist-workspace.toml (workspace root, generated by cargo-dist)
[dist]
cargo-dist-version = "0.28.0"  # or latest
ci = "github"
```

**Tests:**
- `cargo dist plan` runs successfully
- Generated `release.yml` is valid YAML
- `cargo dist build --artifacts=local` builds for host target

---

#### Task 01.02 — Configure cargo-release for version management

> **Priority:** P0  
> **Depends on:** Task 01.01  
> **Blocked by:** nothing

**Files:** `Cargo.toml` (workspace), `crates/unblock-mcp/Cargo.toml`

Requirements:
- Workspace-level config:
  ```toml
  [workspace.metadata.release]
  shared-version = false
  tag-name = "{{crate_name}}-v{{version}}"
  ```
- Package-level config:
  ```toml
  [package.metadata.release]
  tag-name = "unblock-mcp-v{{version}}"
  ```
- Release flow: `cargo release -p unblock-mcp --execute 1.0.0` → version bump → commit → tag → push → triggers cargo-dist

**Tests:**
- `cargo release -p unblock-mcp --dry-run 1.0.0` completes without error

---

#### Task 01.03 — Validate release workflow on all 5 targets

> **Priority:** P1  
> **Depends on:** Task 01.01  
> **Blocked by:** nothing

**File:** `.github/workflows/release.yml` (generated by cargo-dist)

Requirements:
- Push a test tag (e.g., `unblock-mcp-v0.2.1-rc.1`) to trigger the workflow
- Verify all 5 platform builds succeed
- Verify release artefacts are uploaded:
  - `unblock-mcp-x86_64-unknown-linux-musl.tar.xz`
  - `unblock-mcp-aarch64-unknown-linux-musl.tar.xz`
  - `unblock-mcp-x86_64-apple-darwin.tar.xz`
  - `unblock-mcp-aarch64-apple-darwin.tar.xz`
  - `unblock-mcp-x86_64-pc-windows-msvc.zip`
  - `unblock-mcp-installer.sh`
  - `unblock-mcp-installer.ps1`
- Verify installer scripts download and execute correctly
- Delete the test release after validation

**Tests:**
- CI validation — all 5 runners green
- Download and run binary on each platform (manual or CI matrix)

---

### Epic 02 — Shell & PowerShell Installers

**Goal:** One-line install commands for Linux, macOS, and Windows.

**Tools:** cargo-dist (auto-generated)

---

#### Task 02.01 — Validate shell installer

> **Priority:** P1  
> **Depends on:** Task 01.03  
> **Blocked by:** nothing

**File:** Generated by cargo-dist — `unblock-mcp-installer.sh`

Requirements:
- Install command works: `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/websublime/unblock/releases/latest/download/unblock-mcp-installer.sh | sh`
- Installs to `~/.cargo/bin/unblock-mcp` (or platform-appropriate location)
- Binary is executable and functional: `unblock-mcp --version`
- Works on: Ubuntu 22.04+, macOS 13+

**Tests:**
- Manual validation on Linux and macOS
- CI smoke test in release workflow

---

#### Task 02.02 — Validate PowerShell installer

> **Priority:** P2  
> **Depends on:** Task 01.03  
> **Blocked by:** nothing

**File:** Generated by cargo-dist — `unblock-mcp-installer.ps1`

Requirements:
- Install command works: `powershell -c "irm https://github.com/websublime/unblock/releases/latest/download/unblock-mcp-installer.ps1 | iex"`
- Binary is functional: `unblock-mcp --version`

**Tests:**
- Manual validation on Windows

---

### Epic 03 — npm Wrapper

**Goal:** `npx @unblock/cli` downloads and runs the correct platform binary.

**External repo:** `websublime/unblock-npm`

---

#### Task 03.01 — Create npm package scaffold

> **Priority:** P1  
> **Depends on:** Task 01.03  
> **Blocked by:** nothing

**Files:** `package.json`, `bin/cli.js`

Requirements:
- Package name: `@unblock/cli`
- `bin` field points to `bin/cli.js`
- `cli.js` logic:
  1. Detect platform and architecture (`process.platform`, `process.arch`)
  2. Map to release artefact name (e.g., `darwin` + `arm64` → `unblock-mcp-aarch64-apple-darwin.tar.xz`)
  3. Download from GitHub Releases `latest`
  4. Extract binary to `node_modules/.cache/@unblock/cli/`
  5. Execute binary with forwarded args via `child_process.execFileSync()`
- Version tracks `unblock-mcp` version
- `postinstall` script pre-downloads binary at `npm install` time (optional optimization)

```json
{
  "name": "@unblock/cli",
  "version": "1.0.0",
  "description": "Dependency-aware task tracking for AI agents",
  "bin": { "unblock-mcp": "bin/cli.js" },
  "repository": "https://github.com/websublime/unblock",
  "license": "MIT"
}
```

Platform mapping:
| `process.platform` | `process.arch` | Target |
|---|---|---|
| `linux` | `x64` | `x86_64-unknown-linux-musl` |
| `linux` | `arm64` | `aarch64-unknown-linux-musl` |
| `darwin` | `x64` | `x86_64-apple-darwin` |
| `darwin` | `arm64` | `aarch64-apple-darwin` |
| `win32` | `x64` | `x86_64-pc-windows-msvc` |

**Tests:**
- `npx @unblock/cli --version` returns correct version
- Platform detection logic tested on all 5 combinations

---

#### Task 03.02 — npm publish automation

> **Priority:** P2  
> **Depends on:** Task 03.01  
> **Blocked by:** npm access token configured

**File:** `.github/workflows/npm-publish.yml` (in unblock-npm repo)

Requirements:
- Trigger: `workflow_dispatch` with version input (or webhook from main repo release)
- Steps: update `package.json` version → `npm publish --access public`
- Requires `NPM_TOKEN` secret

**Tests:**
- Dry run: `npm publish --dry-run`

---

### Epic 04 — Homebrew Formula

**Goal:** `brew install websublime/tap/unblock` installs the latest release.

**External repo:** `websublime/homebrew-tap`

---

#### Task 04.01 — Create Homebrew tap repository

> **Priority:** P2  
> **Depends on:** Task 01.03  
> **Blocked by:** nothing

**File:** `Formula/unblock.rb` (in `websublime/homebrew-tap`)

Requirements:
- Formula name: `unblock` (maps to binary `unblock-mcp`)
- Downloads platform-appropriate binary from GitHub Releases
- SHA256 verification
- `test` block: `system "#{bin}/unblock-mcp", "--version"`

```ruby
class Unblock < Formula
  desc "Dependency-aware task tracking for AI agents, powered by GitHub"
  homepage "https://github.com/websublime/unblock"
  version "1.0.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/websublime/unblock/releases/download/unblock-mcp-v#{version}/unblock-mcp-aarch64-apple-darwin.tar.xz"
      sha256 "PLACEHOLDER"
    end
    on_intel do
      url "https://github.com/websublime/unblock/releases/download/unblock-mcp-v#{version}/unblock-mcp-x86_64-apple-darwin.tar.xz"
      sha256 "PLACEHOLDER"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/websublime/unblock/releases/download/unblock-mcp-v#{version}/unblock-mcp-aarch64-unknown-linux-musl.tar.xz"
      sha256 "PLACEHOLDER"
    end
    on_intel do
      url "https://github.com/websublime/unblock/releases/download/unblock-mcp-v#{version}/unblock-mcp-x86_64-unknown-linux-musl.tar.xz"
      sha256 "PLACEHOLDER"
    end
  end

  def install
    bin.install "unblock-mcp"
  end

  test do
    system "#{bin}/unblock-mcp", "--version"
  end
end
```

**Tests:**
- `brew install --build-from-source websublime/tap/unblock` succeeds
- `brew test websublime/tap/unblock` passes

---

#### Task 04.02 — Automate formula updates via cargo-dist

> **Priority:** P3  
> **Depends on:** Task 04.01, Task 01.01  
> **Blocked by:** nothing

**File:** `unblock-mcp/Cargo.toml`

Requirements:
- Enable Homebrew publisher in cargo-dist:
  ```toml
  [package.metadata.dist]
  installers = ["shell", "powershell", "homebrew"]
  tap = "websublime/homebrew-tap"
  publish-jobs = ["homebrew"]
  ```
- cargo-dist will auto-update the formula with correct URLs and SHA256 on each release
- Requires `HOMEBREW_TAP_TOKEN` secret (PAT with `repo` scope on the tap repo)

**Tests:**
- Release a test version → verify formula in tap repo was updated

---

### Epic 05 — Materialised Fast Path (Strategy D)

**Goal:** Serve the ready queue instantly on cold start by reading Ready State field values from GitHub, while rebuilding the full graph asynchronously. Reduces cold start from seconds to <500ms.

**Crates:** `unblock-core`, `unblock-mcp`

---

#### Task 05.01 — Define `FastPathReady` type

> **Priority:** P1  
> **Depends on:** nothing  
> **Blocked by:** nothing

**File:** `unblock-core/src/fast_path.rs`

Requirements:
- `FastPathReady` struct: issues that have `ReadyState::Ready` in their Projects V2 field, served directly without graph computation
- Contains: `issues: Vec<IssueSummary>`, `source: FastPathSource`, `stale: bool`
- `FastPathSource` enum: `Field` (from Projects V2 field), `Graph` (from computed graph)
- When `source == Field`, the result is approximate — it reflects the last time the MCP server (or reconcile) wrote the field. It may be stale if external mutations occurred

```rust
#[derive(Debug, Clone, Serialize)]
pub struct FastPathReady {
    pub issues: Vec<IssueSummary>,
    pub source: FastPathSource,
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize)]
pub enum FastPathSource {
    /// Served from Projects V2 Ready State field values (approximate, fast).
    Field,
    /// Served from computed dependency graph (authoritative, slower).
    Graph,
}
```

**Tests:**
- `fast_path_ready_from_field_is_approximate`
- `fast_path_ready_from_graph_is_authoritative`

---

#### Task 05.02 — Implement field-based ready query

> **Priority:** P1  
> **Depends on:** Task 05.01  
> **Blocked by:** nothing

**File:** `unblock-github/src/graphql.rs`

Requirements:
- New GraphQL query: fetch all issues where Ready State field == `ready`
- This is a single-field filter query — much lighter than `fetch_graph_data()` which fetches everything
- Returns `Vec<IssueSummary>` with issue number, title, priority, status
- Query uses Projects V2 field filtering: filter items by field value

```rust
pub async fn fetch_ready_from_field(&self) -> Result<Vec<IssueSummary>, Error> {
    // GraphQL: query project items where ReadyState == "ready"
    // Returns lightweight issue summaries without full graph data
}
```

**Tests (integration):**
- `fetch_ready_from_field_returns_ready_issues`
- `fetch_ready_from_field_excludes_blocked_issues`

---

#### Task 05.03 — Implement fast path in `ready` tool handler

> **Priority:** P1  
> **Depends on:** Task 05.01, Task 05.02  
> **Blocked by:** nothing

**File:** `unblock-mcp/src/tools/ready.rs`

Requirements:
- On cold start (cache is `Empty`):
  1. Immediately fetch ready set from field values via `fetch_ready_from_field()`
  2. Spawn background task: `fetch_graph_data()` → build graph → update cache
  3. Return `FastPathReady { issues, source: Field, stale: false }`
- On warm cache (`Fresh` or `Stale`):
  - Behaviour unchanged — serve from graph
  - Return `FastPathReady { issues, source: Graph, stale: false }`
- The `ready` tool response includes `source` so the agent knows if the result is approximate
- When background graph build completes, subsequent `ready` calls use the graph

**Timing target:** Cold start under 500ms for repos with <500 issues using the fast path.

```rust
pub async fn handle_ready(
    params: ReadyParams,
    state: &ServerState,
) -> Result<ReadyOutput, McpError> {
    match state.cache.get() {
        CacheResult::Empty => {
            // Fast path: serve from fields
            let field_ready = state.github.fetch_ready_from_field().await?;
            // Spawn background graph build
            tokio::spawn(rebuild_graph_async(state.clone()));
            Ok(ReadyOutput::from_fast_path(field_ready))
        }
        CacheResult::Fresh(entry) | CacheResult::Stale(entry) => {
            // Normal path: serve from graph
            Ok(ReadyOutput::from_graph(entry))
        }
    }
}
```

**Tests:**
- `ready_serves_from_field_on_cold_start`
- `ready_serves_from_graph_on_warm_cache`
- `ready_background_build_populates_cache`
- `ready_cold_start_under_500ms` (integration, timing test)

---

#### Task 05.04 — Background graph rebuild function

> **Priority:** P1  
> **Depends on:** Task 05.03  
> **Blocked by:** nothing

**File:** `unblock-mcp/src/tools/ready.rs` (or `src/cache_builder.rs`)

Requirements:
- `async fn rebuild_graph_async(state: ServerState)` — fetches graph data, builds graph, populates cache
- Does NOT hold the cache lock while fetching — the fast path can continue serving
- Logs at `info` level when background build starts and completes
- If fetch fails, logs at `warn` — does not crash. Next `ready` call retries

```rust
async fn rebuild_graph_async(state: ServerState) {
    tracing::info!("Background graph rebuild starting");
    match state.github.fetch_graph_data().await {
        Ok((issues, edges)) => {
            let graph = DependencyGraph::build(&issues, &edges);
            state.cache.update(graph);
            tracing::info!("Background graph rebuild completed");
        }
        Err(e) => {
            tracing::warn!("Background graph rebuild failed: {e}");
        }
    }
}
```

**Tests:**
- `background_rebuild_populates_cache`
- `background_rebuild_failure_does_not_crash`

---

### Epic 06 — GitHub Enterprise Server Support

**Goal:** Full compatibility with GHE Server and GHE Cloud via configurable API URLs.

**Crate:** `unblock-github`

---

#### Task 06.01 — Centralise URL construction

> **Priority:** P1  
> **Depends on:** nothing  
> **Blocked by:** nothing

**File:** `unblock-github/src/client.rs`

Requirements:
- All URL construction goes through `GitHubClient` methods:
  - `rest_url(&self, path: &str) -> String` — `{api_base_url}/{path}`
  - `graphql_url(&self) -> String` — handles GHE Server: if `api_base_url` ends with `/v3`, strip and append `/graphql`. Otherwise `{api_base_url}/graphql`
  - `html_url(&self, path: &str) -> String` — `{github_url}/{path}` (for issue links in comments)
- No hardcoded `api.github.com` strings anywhere except `Config::default()`
- Trailing slash normalisation: `api_base_url` stored without trailing slash

URL resolution by environment:

| Environment | `GITHUB_API_URL` | REST | GraphQL |
|---|---|---|---|
| github.com | `https://api.github.com` | `{base}/repos/o/r/issues` | `{base}/graphql` |
| GHE Server | `https://<host>/api/v3` | `{base}/repos/o/r/issues` | `https://<host>/api/graphql` |
| GHE Cloud | `https://api.<host>` | `{base}/repos/o/r/issues` | `{base}/graphql` |

```rust
impl GitHubClient {
    fn rest_url(&self, path: &str) -> String {
        format!("{}/{}", self.api_base_url, path)
    }

    fn graphql_url(&self) -> String {
        if self.api_base_url.ends_with("/v3") {
            let base = self.api_base_url.trim_end_matches("/v3");
            format!("{}/graphql", base)
        } else {
            format!("{}/graphql", self.api_base_url)
        }
    }

    fn html_url(&self, path: &str) -> String {
        format!("{}/{}", self.github_url, path)
    }
}
```

**Tests:**
- `rest_url_github_com`
- `rest_url_ghe_server`
- `graphql_url_github_com` — `https://api.github.com/graphql`
- `graphql_url_ghe_server` — strips `/v3`: `https://ghe.corp.com/api/graphql`
- `graphql_url_ghe_cloud` — `https://api.corp.github.com/graphql`
- `html_url_uses_github_url_config`
- `trailing_slash_normalised`

---

#### Task 06.02 — Add `GITHUB_URL` to Config

> **Priority:** P1  
> **Depends on:** Task 06.01  
> **Blocked by:** nothing

**File:** `unblock-core/src/config.rs`

Requirements:
- Add `github_url: String` to `Config` (default: `"https://github.com"`)
- Loaded from `GITHUB_URL` environment variable
- Used for HTML links in comments and audit trails (e.g., `https://ghe.corp.com/owner/repo/issues/42`)
- Trailing slash normalisation

```rust
pub struct Config {
    // ... existing fields
    pub github_url: String,     // GITHUB_URL (default: "https://github.com")
}
```

**Tests:**
- `config_loads_github_url_from_env`
- `config_defaults_github_url_to_github_com`

---

#### Task 06.03 — GHE integration test

> **Priority:** P2  
> **Depends on:** Task 06.01, Task 06.02  
> **Blocked by:** Access to a GHE Server instance (can be mocked)

**File:** `tests/ghe_integration.rs`

Requirements:
- Test URL construction for GHE Server and GHE Cloud environments
- Test `graphql_url()` correctly strips `/v3` for GHE Server
- Test full tool flow with mocked GHE-style responses
- If a real GHE instance is available, run live integration tests (gated by env var)

**Tests:**
- `ghe_server_graphql_url_correct`
- `ghe_server_rest_url_correct`
- `ghe_cloud_graphql_url_correct`
- `ghe_full_flow_mock` — mock GHE responses, verify tool chain works

---

### Epic 07 — GitHub App Authentication

**Goal:** Support GitHub App authentication for higher rate limits (15,000/hour), org-wide installation, and bot identity (`unblock[bot]`).

**Crate:** `unblock-github`

---

#### Task 07.01 — Define `GitHubAuth` trait

> **Priority:** P1  
> **Depends on:** nothing  
> **Blocked by:** nothing

**File:** `unblock-github/src/auth.rs`

Requirements:
- `GitHubAuth` trait: `async fn token(&self) -> Result<String, Error>` — returns a valid token for the current request
- `Send + Sync` bounds for use in `Arc<dyn GitHubAuth>`
- Two implementations:
  - `TokenAuth` — wraps a static PAT. `token()` returns it directly
  - `AppAuth` — generates installation tokens from App credentials. Caches token until expiry
- `TokenAuth` is the existing behaviour, extracted into the trait

```rust
#[async_trait]
pub trait GitHubAuth: Send + Sync + std::fmt::Debug {
    /// Returns a valid GitHub API token.
    async fn token(&self) -> Result<String, Error>;
}

#[derive(Debug)]
pub struct TokenAuth {
    token: String,
}

impl TokenAuth {
    pub fn new(token: String) -> Self { Self { token } }
}

#[async_trait]
impl GitHubAuth for TokenAuth {
    async fn token(&self) -> Result<String, Error> {
        Ok(self.token.clone())
    }
}
```

**Tests:**
- `token_auth_returns_configured_token`

---

#### Task 07.02 — Implement `AppAuth` (GitHub App installation tokens)

> **Priority:** P1  
> **Depends on:** Task 07.01  
> **Blocked by:** nothing

**File:** `unblock-github/src/auth.rs`

Requirements:
- `AppAuth` struct: `app_id: u64`, `private_key: String`, `installation_id: u64`, `cached_token: RwLock<Option<CachedToken>>`
- `CachedToken`: `token: String`, `expires_at: DateTime<Utc>`
- JWT generation: sign with RS256 using the App's private key. Claims: `iss` = app_id, `iat` = now - 60s, `exp` = now + 600s
- Token generation: `POST /app/installations/{installation_id}/access_tokens` with JWT auth
- Token caching: reuse until 5 minutes before expiry. GitHub installation tokens expire after 1 hour
- Thread-safe: `RwLock` for cached token, read path is lock-free when token is fresh
- Library: `jsonwebtoken` for JWT signing

```rust
#[derive(Debug)]
pub struct AppAuth {
    app_id: u64,
    private_key: String,
    installation_id: u64,
    http: reqwest::Client,
    api_base_url: String,
    cached_token: RwLock<Option<CachedToken>>,
}

struct CachedToken {
    token: String,
    expires_at: DateTime<Utc>,
}

#[async_trait]
impl GitHubAuth for AppAuth {
    async fn token(&self) -> Result<String, Error> {
        // Check cache
        if let Some(cached) = self.cached_token.read().await.as_ref() {
            if cached.expires_at > Utc::now() + Duration::minutes(5) {
                return Ok(cached.token.clone());
            }
        }
        // Generate new token
        let jwt = self.generate_jwt()?;
        let token = self.create_installation_token(&jwt).await?;
        // Cache
        *self.cached_token.write().await = Some(token.clone());
        Ok(token.token)
    }
}
```

**Tests:**
- `app_auth_generates_valid_jwt`
- `app_auth_caches_token_until_near_expiry`
- `app_auth_refreshes_expired_token`
- `app_auth_concurrent_refresh_only_once` — verify no thundering herd

---

#### Task 07.03 — Integrate `GitHubAuth` with `GitHubClient`

> **Priority:** P1  
> **Depends on:** Task 07.01, Task 07.02  
> **Blocked by:** nothing

**File:** `unblock-github/src/client.rs`

Requirements:
- Replace `token: String` field with `auth: Arc<dyn GitHubAuth>`
- In `graphql_request()` and `rest_request()`: `let token = self.auth.token().await?;`
- `Authorization: Bearer {token}` header set per-request (token may change between requests for App auth)
- Backwards-compatible: existing PAT usage creates `TokenAuth` — no API change for callers

```rust
pub struct GitHubClient {
    http: reqwest::Client,
    auth: Arc<dyn GitHubAuth>,
    api_base_url: String,
    github_url: String,
    // ... other fields
}
```

**Tests:**
- `client_uses_token_auth_by_default`
- `client_uses_app_auth_when_configured`
- `client_refreshes_app_token_on_expiry`

---

#### Task 07.04 — Auth strategy selection at startup

> **Priority:** P1  
> **Depends on:** Task 07.03  
> **Blocked by:** nothing

**File:** `unblock-mcp/src/main.rs`, `unblock-core/src/config.rs`

Requirements:
- New environment variables:
  - `GITHUB_APP_ID` — App ID
  - `GITHUB_APP_PRIVATE_KEY` — PEM-encoded private key (or path to file)
  - `GITHUB_APP_INSTALLATION_ID` — Installation ID
- Detection priority:
  1. If all 3 App vars present → `AppAuth`
  2. If `GITHUB_TOKEN` present → `TokenAuth`
  3. Neither → hard error
- Log which auth strategy is active at startup (at `info` level)
- Private key handling: if value starts with `-----BEGIN`, treat as inline PEM. Otherwise, treat as file path and read

```rust
fn select_auth(config: &Config) -> Result<Arc<dyn GitHubAuth>, Error> {
    if let (Some(app_id), Some(key), Some(install_id)) = (
        config.app_id,
        config.app_private_key.as_ref(),
        config.app_installation_id,
    ) {
        Ok(Arc::new(AppAuth::new(app_id, key, install_id, &config.api_base_url)?))
    } else {
        Ok(Arc::new(TokenAuth::new(config.token.clone())))
    }
}
```

**Tests:**
- `auth_selection_prefers_app_when_all_vars_present`
- `auth_selection_falls_back_to_token`
- `auth_selection_fails_when_no_auth_configured`
- `private_key_from_inline_pem`
- `private_key_from_file_path`

---

### Epic 08 — Coverage & Quality Hardening

**Goal:** 100% coverage target, property tests for graph invariants, comprehensive integration test suite.

**Crate:** All

---

#### Task 08.01 — Property tests for graph invariants

> **Priority:** P2  
> **Depends on:** nothing  
> **Blocked by:** nothing

**File:** `unblock-core/tests/proptest_graph.rs`

Requirements:
- Use `proptest` crate for property-based testing
- Invariants to validate:
  1. **Ready set never contains blocked issues** — for any graph, no issue in the ready set has an open blocker
  2. **Cascade produces correct promotions** — closing an issue and recomputing: all dependents whose blockers are now all closed appear in the ready set
  3. **Cycle detection is sound** — if `detect_all_cycles()` returns empty, `would_create_cycle(a, b)` is false for any existing edge `a→b`
  4. **Cycle detection is complete** — if a cycle exists, `detect_all_cycles()` finds it
  5. **Ready set is stable** — computing ready set twice on the same graph yields the same result

```rust
proptest! {
    #[test]
    fn ready_set_never_contains_blocked_issues(
        issues in vec(arb_issue(), 1..100),
        edges in vec(arb_edge(), 0..200),
    ) {
        let graph = DependencyGraph::build(&issues, &edges);
        let ready = graph.compute_ready_set(&issues);
        for issue in &ready {
            // No open blocker exists for this issue
        }
    }
}
```

**Tests:**
- 5 property tests as described above
- Run with `PROPTEST_CASES=10000` in CI

---

#### Task 08.02 — Integration test suite hardening

> **Priority:** P2  
> **Depends on:** nothing  
> **Blocked by:** nothing

**File:** `tests/integration/`

Requirements:
- Verify all 20 tools work end-to-end against a test GitHub repo
- Test cross-repo dependency operations
- Test circuit breaker and retry behaviour with mocked failures
- Test reconcile with synthetic drift scenarios
- Test doctor with various misconfiguration states
- All integration tests gated by `GITHUB_TOKEN` env var — skip when not set

**Tests:**
- Full tool coverage in integration tests

---

#### Task 08.03 — Coverage enforcement in CI

> **Priority:** P3  
> **Depends on:** Task 08.01, Task 08.02  
> **Blocked by:** nothing

**File:** `.github/workflows/ci.yml`

Requirements:
- Add coverage job using `cargo-tarpaulin`
- Fail CI if coverage drops below 80% (Phase 02 target) or 95% (Phase 03 target, aspirational)
- Upload coverage to Codecov
- Coverage excludes: test modules, generated code, `main.rs` bootstrap

---

### Epic 09 — v1.0.0 Release

**Goal:** Tag and release v1.0.0 with all distribution channels active.

---

#### Task 09.01 — Version bump and changelog

> **Priority:** P0  
> **Depends on:** All other epics  
> **Blocked by:** nothing

**File:** `crates/unblock-mcp/Cargo.toml`, `CHANGELOG.md`

Requirements:
- Bump version to `1.0.0`
- Write CHANGELOG.md covering all Phase 01, 02, and 03 features
- Conventional Commits format
- Semantic versioning: v1.0.0 = first stable release, public API committed

---

#### Task 09.02 — Release and distribution validation

> **Priority:** P0  
> **Depends on:** Task 09.01  
> **Blocked by:** nothing

Requirements:
- `cargo release -p unblock-mcp --execute 1.0.0`
- Verify all 5 platform builds succeed
- Verify all distribution channels:
  - GitHub Releases: binaries + installers uploaded
  - npm: `npx @unblock/cli --version` returns `1.0.0`
  - Homebrew: `brew install websublime/tap/unblock && unblock-mcp --version`
  - Shell installer: `curl ... | sh && unblock-mcp --version`
- Verify MCP client integration:
  - Claude Code: `mcp.json` config works
  - GitHub Copilot CLI: config works

---

#### Task 09.03 — Update README with install instructions

> **Priority:** P1  
> **Depends on:** Task 09.02  
> **Blocked by:** nothing

**File:** `README.md`

Requirements:
- Installation section with all methods:
  ```bash
  # Shell installer (Linux/macOS)
  curl --proto '=https' --tlsv1.2 -LsSf https://github.com/websublime/unblock/releases/latest/download/unblock-mcp-installer.sh | sh

  # npm
  npx @unblock/cli

  # Homebrew
  brew install websublime/tap/unblock

  # Cargo
  cargo install unblock-mcp
  ```
- MCP client configuration examples for Claude Code and GitHub Copilot
- GHE configuration example

---

## 6. Definition of Done

Phase 03 is complete when:

1. **All 9 epics are implemented** — cargo-dist, installers, npm, Homebrew, fast path, GHE, App auth, quality, release
2. **Quality gate passes:**
   ```bash
   cargo fmt --check --all                                    # zero diffs
   cargo clippy --workspace --all-targets -- -D warnings      # zero warnings
   cargo test --workspace                                     # all pass
   cargo doc --no-deps --workspace                            # zero warnings
   ```
3. **Cross-platform binaries** pass CI on all 5 target platforms
4. **Cold start under 500ms** for repos with <500 issues using materialised fast path (integration benchmark)
5. **GHE Server** integration tests pass on configurable API URL
6. **GitHub App auth** correctly generates and caches installation tokens
7. **All distribution channels** functional: GitHub Releases, shell installer, PowerShell installer, npm, Homebrew
8. **Coverage target:** 100% for new code (Phase 03), >80% overall
9. **Property tests** pass with 10,000 cases per invariant
10. **v1.0.0 tag** published, release notes written, all artefacts uploaded
11. **README** updated with installation instructions for all channels

---

*This plan defines what to build in Phase 03. The why is in the PRD §7 Phase 03. The how is in the SPEC §4.7, §5.6, §14, §16. Distribution details are in the CI/CD architecture doc.*
