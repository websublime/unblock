# Unblock — LLM Agent Research

**Exploring a custom autonomous agent: event-driven, self-hosted LLM, remote MCP tools.**

| | |
|---|---|
| **Date** | April 2026 |
| **Status** | Research — not committed |
| **Author** | Miguel Ramos |
| **Org** | websublime |
| **Depends on** | `unblock-architecture-github.md` v1.2.0-draft, `unblock-remote-mcp-research.md` |

---

## Table of Contents

1. [Problem](#1-problem)
2. [Concept](#2-concept)
3. [Relationship with the Remote MCP](#3-relationship-with-the-remote-mcp)
4. [The Two Agent Flows](#4-the-two-agent-flows)
5. [LLM Selection](#5-llm-selection)
6. [Agent Loop Design](#6-agent-loop-design)
7. [Workspace Structure](#7-workspace-structure)
8. [Auth Model](#8-auth-model)
9. [Filesystem Access](#9-filesystem-access)
10. [Prompt Design](#10-prompt-design)
11. [Output Format Enforcement](#11-output-format-enforcement)
12. [Webhook Routing](#12-webhook-routing)
13. [Idempotency](#13-idempotency)
14. [Quality Considerations](#14-quality-considerations)
15. [Cost Model](#15-cost-model)
16. [Deployment](#16-deployment)
17. [GHE Support](#17-ghe-support)
18. [Licensing Implications](#18-licensing-implications)
19. [Alternatives Evaluated](#19-alternatives-evaluated)
20. [What Does Not Change](#20-what-does-not-change)
21. [Open Questions](#21-open-questions)

---

## 1. Problem

The current Unblock pipeline requires a human to open a session — Claude Code, Copilot agent
mode, or any MCP-compatible editor — to trigger investigation and code review. Two high-value
phases remain entirely manual:

**Issue Investigation** — when an issue is created, nobody investigates it until a developer or
agent runs `/start-task`. The gap analysis, file discovery, approach suggestion, and risk
identification happen at implementation time — the most expensive moment. By then, the
developer has already claimed the issue, opened a branch, and committed context. Finding a
dependency gap or an ambiguous acceptance criterion at that point means rework.

**PR Code Review** — the existing `unblock-review.yml` Action triggers on the `needs-review`
label on the GitHub Issue. This label is added by the implementing agent after pushing. The
review is agent-driven but still requires a prior human session (`/start-task`) to reach the
push. A PR opened by any other path — Copilot Coding Agent, Dependabot, a human developer —
gets no structured Unblock review unless someone manually triggers it.

The question: can these two phases run **autonomously**, without a human session, using a
self-hosted or API-accessed LLM, connected to the Unblock tool layer via the remote MCP?

---

## 2. Concept

The `unblock-agent` is an autonomous service that:

1. Receives GitHub webhook events
2. Dispatches the appropriate agent flow based on event type and label
3. Runs an LLM-powered agent loop with access to Unblock MCP tools and GitHub content
4. Posts structured comments to GitHub Issues and PR reviews
5. Stops — no implementation, no branch creation, no merging

It is **not** a coding agent. It does not write code. It front-loads the cognitive work that
today happens at the start of implementation (investigation) and after implementation
(review), turning them into background processes that complete before a human or agent
touches the keyboard.

### 2.1 Design Principles

| Principle | Meaning |
|---|---|
| Read-only except for comments | The agent posts comments and PR reviews. It never creates branches, claims issues, or modifies code |
| Idempotent by design | If a structured comment already exists, the agent skips and exits. No duplicate comments |
| Ephemeral per event | Each webhook triggers one agent run. No persistent process state between runs |
| GitHub is still the source of truth | The agent reads from GitHub (via MCP tools and Contents API), writes comments to GitHub. Zero custom storage |
| Self-hosted LLM optional | The agent is designed for Mistral API by default but can be pointed at any OpenAI-compatible endpoint, including a self-hosted vLLM instance |
| Separation from the pipeline | The agent enriches GitHub issues before `/start-task` is invoked. It never replaces the human-in-the-loop pipeline phases |

---

## 3. Relationship with the Remote MCP

The `unblock-agent` is a **client** of `unblock-mcp-remote`. It does not embed tool logic
and does not depend on `unblock-tools` directly. All Unblock tool calls go through the
Streamable HTTP endpoint defined in the remote MCP research.

```
GitHub Webhooks (issues.labeled, pull_request.opened)
        │
        ▼
┌───────────────────────────────────────────────────────┐
│                 unblock-agent service                  │
│                                                        │
│  ┌──────────────────┐    ┌──────────────────────────┐  │
│  │  Webhook Router  │    │     Agent Loop (rig)     │  │
│  │  (Axum)          │───►│  Mistral / Codestral API │  │
│  └──────────────────┘    └────────────┬─────────────┘  │
│                                       │                 │
│                          ┌────────────▼─────────────┐  │
│                          │    MCP HTTP Client       │  │
│                          │    POST /mcp             │  │
│                          └────────────┬─────────────┘  │
└───────────────────────────────────────┼────────────────┘
                                        │ Streamable HTTP
                                        ▼
                          ┌─────────────────────────────┐
                          │     unblock-mcp-remote      │
                          │     (always running)        │
                          │     SharedGraphCache        │
                          └────────────┬────────────────┘
                                       │ HTTPS
                                       ▼
                               GitHub API (GraphQL + REST)
```

### 3.1 Why not embed `unblock-tools` directly

The agent service could depend on `unblock-tools` and call tool functions directly, skipping
the HTTP layer. This would be faster but wrong:

- The `SharedGraphCache` in `unblock-mcp-remote` is already warm from prior sessions.
  Bypassing the remote means a cold graph rebuild on every agent run.
- The remote MCP handles auth (token validation, identity caching). Duplicating this in the
  agent service creates a second auth path to maintain.
- The remote MCP is the canonical tool interface for all clients — Claude Code, Copilot,
  GitHub Actions, and the agent service. They must all use the same code path.

The HTTP overhead is negligible: the agent and the remote MCP are on the same network
(same Fly.io region or same Docker network). Round-trip is <1ms per tool call.

### 3.2 Shared infrastructure

The agent service and `unblock-mcp-remote` can be co-deployed as separate containers on
the same host or Fly.io app. They share no process memory — the cache lives in
`unblock-mcp-remote`, the LLM loop lives in `unblock-agent`. Each restarts independently.

The webhook receiver in `unblock-mcp-remote` (§8 of remote MCP research) invalidates the
graph cache when issues change. The agent service **also** needs to receive webhooks for
dispatch decisions. Two options:

**Option A — Separate webhook endpoints:**
`unblock-mcp-remote` receives `POST /webhooks/github` for cache invalidation.
`unblock-agent` receives its own `POST /webhooks/github` on a different port/service.
GitHub webhook is configured to send to both URLs. Simple, independent.

**Option B — Shared webhook receiver:**
A single endpoint in `unblock-mcp-remote` receives the webhook, invalidates the cache,
and forwards the payload to `unblock-agent` via an internal HTTP call.
Adds coupling. Rejected.

**Decision: Option A.** Two independent webhook subscriptions. GitHub supports multiple
webhook endpoints per repository. Each service manages its own concerns.

---

## 4. The Two Agent Flows

### 4.1 Flow 1 — Issue Investigation (Sherlock)

**Trigger:** `issues.labeled` where `label.name == "needs-investigation"`

**Rationale for opt-in label trigger:** Running investigation on every `issues.opened` is
expensive at scale and noisy for trivial issues (typos, simple config changes). The
`needs-investigation` label is added manually or automatically by Fernando during
`/create-tasks`. Issues that genuinely need upfront investigation get it; simple issues skip
it. The label can also be added retroactively — the agent checks idempotency before running.

**Agent responsibilities:**

1. Call `show {N} --include_comments` via MCP — read issue body and check for existing
   `INVESTIGATION:` comment. If found, exit immediately.
2. Parse issue body: Description, Design Notes, Acceptance Criteria.
3. If issue is a Sub-Issue, call `show {parent}` to get epic-level context.
4. Discover relevant files via GitHub Contents API (directory listing) and GitHub Search API
   (code search for symbols mentioned in the issue).
5. Fetch file content for the top N relevant files via GitHub Contents API.
6. Call Codestral with the full context. Produce a structured investigation.
7. Call `comment {N}` via MCP with the `INVESTIGATION:` formatted output.
8. Remove the `needs-investigation` label via GitHub REST API.

**Sequence:**

```
issues.labeled (needs-investigation)
        │
        ▼
unblock-agent webhook handler
        │
        ├── MCP: show {N} --include_comments
        │     └── INVESTIGATION: comment exists? → EXIT (idempotent)
        │
        ├── GitHub API: fetch file tree (GET /repos/{r}/git/trees/{sha}?recursive=1)
        │
        ├── LLM: "Which files are relevant to this issue?" → file list
        │
        ├── GitHub API: fetch file contents (GET /repos/{r}/contents/{path})
        │     (parallel, max 10 files, max 500 lines each, truncated)
        │
        ├── LLM: full investigation with file context
        │     → INVESTIGATION: comment body
        │
        ├── MCP: comment {N} "{INVESTIGATION: ...}"
        │
        └── GitHub REST: remove label "needs-investigation"
```

**Output on the issue:**

```
INVESTIGATION:

Root cause: {summary of what the issue requires and why}

Files:
- src/graph.rs:142 — dependency resolution entry point; extend GraphEngine::resolve()
- src/cache.rs:89  — TTL invalidation; order matters relative to graph rebuild
- src/types.rs:34  — DriftKind enum; add new variant here

Approach:
1. Add variant to DriftKind in unblock-core/src/types.rs
2. Extend GraphEngine::resolve() in unblock-core/src/graph.rs to handle the new case
3. Add reconcile branch in ReconcileEngine::analyse()
4. Update ReconcileEngine tests in tests/reconcile_tests.rs

Risks:
- Cache invalidation order in cache.rs:89 must be preserved — rebuild before Ready State write
- Concurrent access during reconcile window (see RwLock in cache.rs:112)
- No integration test for this drift type yet — needs to be added

Related tests:
- tests/graph_tests.rs::test_resolution_cycle
- tests/reconcile_tests.rs::test_analyse_drift

Gaps in acceptance criteria:
- Criterion 3 ("updates Ready State field") does not specify behaviour when GitHub API
  is unavailable during the update. Clarify: fail silently or propagate error?
```

### 4.2 Flow 2 — PR Code Review (Linus)

**Trigger:** `pull_request.opened` or `pull_request.ready_for_review` (draft → ready)

**Rationale:** Any PR — whether opened by a human, Copilot Coding Agent, or the Unblock
pipeline — gets a structured review against the Unblock comment trail. The review agent
does not need to know who opened the PR. It reconstructs context from GitHub alone.

**Agent responsibilities:**

1. Extract linked issue number from PR body (`Closes #N`, `Fixes #N`, `Resolves #N`). If
   none found, post a comment requesting a link and exit.
2. Call `show {N} --include_comments` via MCP — read issue body and full comment trail.
3. Verify a `COMPLETED:` comment exists. If not, post a comment noting the issue is not
   marked complete and exit.
4. Fetch the PR diff via GitHub REST API (`GET /repos/{r}/pulls/{pr}/files`).
5. Call Codestral with issue context + comment trail + diff. Produce a structured review.
6. Post `REVIEW:` comment on the **GitHub Issue** via MCP `comment` tool.
7. Submit a formal GitHub PR Review via REST API (`POST /repos/{r}/pulls/{pr}/reviews`)
   with `event: "COMMENT"` (not APPROVE or REQUEST_CHANGES — the agent does not block merges).

**Sequence:**

```
pull_request.opened (or ready_for_review)
        │
        ▼
unblock-agent webhook handler
        │
        ├── extract linked issue from PR body → not found? post comment + EXIT
        │
        ├── MCP: show {N} --include_comments
        │     ├── REVIEW: comment exists? → EXIT (idempotent)
        │     └── COMPLETED: comment missing? → post "not complete" comment + EXIT
        │
        ├── GitHub REST: GET /repos/{r}/pulls/{PR}/files → diff per file
        │
        ├── LLM: review diff against acceptance criteria + comment trail
        │     → REVIEW: comment body
        │     → list of inline review comments (file, line, message)
        │
        ├── MCP: comment {N} "{REVIEW: ...}"   ← on the Issue
        │
        └── GitHub REST: POST /repos/{r}/pulls/{PR}/reviews
              event: "COMMENT"
              body: summary
              comments: [{path, line, body}, ...]  ← inline on the PR
```

**Output — REVIEW comment on issue:**

```
REVIEW:

Verdict: APPROVE

Summary: Implementation matches acceptance criteria. Two non-blocking observations noted
as inline PR comments.

Criteria check:
- [GOOD]     Criterion 1: DriftKind variant added correctly in types.rs
- [GOOD]     Criterion 2: ReconcileEngine::analyse() handles new drift type
- [WARNING]  Criterion 3: Ready State update on GitHub API failure — implementation
             silently swallows the error. Criterion was ambiguous; implementation chose
             silent failure. Acceptable but should be documented as a DEVIATION.
- [GOOD]     Criterion 4: Tests added for new drift type — 3 cases covered

Findings:
- [WARNING] src/graph.rs:201 — error path returns stale cache without stale:true flag
- [INFO]    src/types.rs:40  — new enum variant could derive Copy given its size
```

---

## 5. LLM Selection

### 5.1 Task Profile

Both flows (investigation and review) are **code understanding** tasks, not code generation.
The requirements are:

- Read and understand Rust code (idiomatic patterns, lifetimes, trait bounds)
- Follow structured system prompts precisely
- Produce output in a rigid format (`INVESTIGATION:`, `REVIEW:` comment syntax)
- Tool calling fidelity (JSON schema adherence for MCP tool invocations)
- Reasoning about acceptance criteria gaps and implementation mismatches

This profile favours code-specialist models over general-purpose large models.

### 5.2 Model Comparison

| Model | Params | Rust code quality | Structured output | Tool calling | Self-host | API cost (input/output per 1M tokens) |
|---|---|---|---|---|---|---|
| **Codestral** (Mistral) | 22B | ★★★★★ | ★★★★☆ | ★★★★☆ | Yes (vLLM) | €0.03 / €0.10 |
| **Mistral Small 3.1** | 24B | ★★★★☆ | ★★★★★ | ★★★★★ | Yes (vLLM) | €0.10 / €0.30 |
| **Mistral Large** | 123B | ★★★★☆ | ★★★★★ | ★★★★★ | Heavy | €2.00 / €6.00 |
| **DeepSeek Coder V2 Lite** | 16B | ★★★★★ | ★★★★☆ | ★★★☆☆ | Yes | €0.07 / €0.28 |
| **Qwen2.5-Coder 32B** | 32B | ★★★★★ | ★★★★☆ | ★★★★☆ | Yes | Self-host only |
| **Llama 3.3 70B** | 70B | ★★★★☆ | ★★★★☆ | ★★★★☆ | Heavy | Via providers |
| **Claude Sonnet 4.5** (reference) | — | ★★★★★ | ★★★★★ | ★★★★★ | No | €3.00 / €15.00 |

### 5.3 Decision: Codestral as Default, Mistral Small as Fallback

**Codestral** is the primary choice for both flows:

- Designed specifically for code tasks by Mistral. Strong Rust comprehension — trained on
  public repositories with significant Rust representation.
- 22B fits on a single A100 80GB or two RTX 3090s for self-hosted deployment.
- Cheapest per-token cost in the Mistral family for code tasks.
- Mistral API is OpenAI-compatible — zero adapter code needed with `rig`.

**Mistral Small 3.1** as fallback for structured output sensitivity: if prompt engineering
experiments show Codestral drifting from the required `INVESTIGATION:` / `REVIEW:` format,
Mistral Small has stronger instruction following. Cost is 3× higher but still cheap.

**Why not Mistral Large:** The quality gain over Codestral for code understanding does not
justify 67× the token cost. Reserved for fine-tuning evaluation if quality falls short.

**Why not DeepSeek Coder V2 Lite:** Tool calling reliability is lower than Mistral models.
MCP tool dispatch depends on schema-correct JSON — a model that hallucinates tool parameters
produces corrupt state in GitHub. Not acceptable for a write-capable agent.

**Why not Claude:** The agent's value proposition includes data sovereignty and predictable
cost. Routing agent runs through Anthropic API defeats both. Claude remains the right
choice for interactive developer sessions via Claude Code.

### 5.4 Provider Strategy

```
Phase 4 initial:   Mistral API (Codestral) — pay-per-use, zero infra overhead
Phase 4 growth:    Evaluate self-hosted Codestral via vLLM if monthly spend > €500
Phase 5:           Fine-tune Codestral on Unblock comment history (500+ issues minimum)
```

Self-hosting gates: vLLM with Codestral 22B requires ~45GB VRAM. A single A100 80GB on
Lambda Labs or Vast.ai costs ~$1.50/hour. At 10,000 agent runs/month with average 50K
tokens each, the Mistral API cost is ~€15/month — self-hosting is not justified until
volume is ~100× that.

---

## 6. Agent Loop Design

### 6.1 Framework: `rig`

`rig` is a Rust-native LLM framework with tool calling support and OpenAI-compatible
provider abstraction. It fits the existing stack (Rust, tokio, async) without introducing
a Python or TypeScript runtime.

```toml
# crates/unblock-agent/Cargo.toml
[dependencies]
rig-core = "0.1"
```

The `rig` agent loop handles:
- System prompt injection
- Tool schema registration (JSON Schema via `schemars`)
- Tool call dispatch and result injection
- Multi-turn conversation until `stop_reason: EndTurn`
- Max-turn safety limits

### 6.2 MCP Tool Wrappers

The agent does not call `unblock-tools` functions directly. It invokes them via HTTP MCP.
Each MCP tool is wrapped as a `rig` tool — a Rust struct implementing `rig::Tool`:

```rust
// crates/unblock-agent/src/tools/mcp.rs

use rig::{completion::ToolDefinition, tool::Tool};
use serde::{Deserialize, Serialize};
use schemars::JsonSchema;

pub struct McpToolClient {
    http:    reqwest::Client,
    base:    String,         // "https://unblock.example.com/mcp"
    token:   SecretString,   // GitHub service account token
    session: SessionId,      // established on initialize
}

/// Wrapper for the `show` MCP tool.
pub struct ShowTool {
    client: Arc<McpToolClient>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ShowParams {
    /// Issue number to show
    pub id: u64,
    /// Include comment trail
    #[serde(default)]
    pub include_comments: bool,
}

impl Tool for ShowTool {
    const NAME: &'static str = "show";

    type Error = AgentError;
    type Args  = ShowParams;
    type Output = serde_json::Value;

    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name:        Self::NAME.into(),
            description: "Read a GitHub Issue with its body sections and comment trail".into(),
            parameters:  schemars::schema_for!(ShowParams),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.client.call_tool("show", serde_json::to_value(args)?).await
    }
}
```

All 18 MCP tools can be wrapped this way. For the investigation flow, only `show` and
`comment` are needed. For the review flow, only `show` and `comment` are needed. The agent
never calls `claim`, `close`, `create`, or any write tool beyond `comment`.

### 6.3 Agent Construction

```rust
// crates/unblock-agent/src/agents/sherlock.rs

use rig::providers::openai; // Mistral API is OpenAI-compatible

pub async fn run_investigation(
    ctx:    &AgentContext,
    issue:  u64,
    files:  Vec<FileContent>,
) -> Result<(), AgentError> {

    let mistral = openai::Client::from_url(
        ctx.mistral_api_key.expose_secret(),
        "https://api.mistral.ai/v1",
    );

    let agent = mistral
        .agent("codestral-latest")
        .preamble(SHERLOCK_SYSTEM_PROMPT)
        .max_tokens(4096)
        .tool(ShowTool::new(ctx.mcp_client.clone()))
        .tool(CommentTool::new(ctx.mcp_client.clone()))
        .build();

    // Inject the issue and file context as the user turn
    let prompt = format_sherlock_prompt(issue, &files);

    agent.prompt(&prompt).await?;
    // The agent loop ends when the model calls `comment` and then stops (EndTurn).
    // SHERLOCK_SYSTEM_PROMPT instructs: after posting the INVESTIGATION comment, stop.

    Ok(())
}
```

### 6.4 Safety Limits

```rust
pub struct AgentConfig {
    /// Maximum LLM turns before aborting. Prevents infinite loops.
    pub max_turns: u32,          // default: 15

    /// Maximum files fetched per investigation run.
    pub max_files: usize,        // default: 12

    /// Maximum lines per file before truncation.
    pub max_lines_per_file: usize, // default: 400

    /// Maximum total input tokens across all turns.
    pub max_input_tokens: u32,   // default: 80_000

    /// Timeout per agent run.
    pub run_timeout: Duration,   // default: 120s
}
```

If `max_turns` is exceeded, the agent posts a failure comment:

```
INVESTIGATION: [INCOMPLETE]

The investigation agent exceeded the turn limit (15 turns) before completing.
Partial findings:

Files identified: src/graph.rs, src/cache.rs
Approach: incomplete

Re-trigger by removing and re-adding the "needs-investigation" label.
```

---

## 7. Workspace Structure

### 7.1 New crate: `unblock-agent`

```
crates/
├── unblock-core/           ← zero changes
├── unblock-github/         ← zero changes
├── unblock-tools/          ← zero changes (agent does not depend on this)
├── unblock-mcp/            ← zero changes
├── unblock-mcp-remote/     ← zero changes
├── unblock-app/            ← zero changes
└── unblock-agent/          ← NEW
    └── src/
        ├── main.rs          ← Axum server, webhook endpoint
        ├── config.rs        ← AgentConfig, env vars
        ├── webhook.rs       ← routing: event + label → dispatch
        ├── agents/
        │   ├── mod.rs
        │   ├── sherlock.rs  ← investigation agent loop
        │   └── linus.rs     ← review agent loop
        ├── tools/
        │   ├── mod.rs
        │   ├── mcp.rs       ← MCP HTTP client + tool wrappers (show, comment)
        │   └── github.rs    ← GitHub Contents API, PR diff, PR review submit
        ├── prompts/
        │   ├── mod.rs
        │   ├── sherlock.rs  ← SHERLOCK_SYSTEM_PROMPT (compiled in)
        │   └── linus.rs     ← LINUS_SYSTEM_PROMPT (compiled in)
        ├── idempotency.rs   ← comment trail parsing, INVESTIGATION/REVIEW detection
        └── errors.rs        ← AgentError
```

### 7.2 `unblock-agent/Cargo.toml`

```toml
[package]
name    = "unblock-agent"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "unblock-agent"
path = "src/main.rs"

[dependencies]
# HTTP server (webhook receiver)
axum          = { workspace = true }
tower         = { workspace = true }
tower-http    = { workspace = true }
tokio         = { workspace = true }

# LLM agent loop
rig-core      = "0.1"

# GitHub API (Contents, PR diff, PR reviews)
reqwest       = { workspace = true }
serde         = { workspace = true }
serde_json    = { workspace = true }
schemars      = { workspace = true }

# Auth + secrets
secrecy       = { workspace = true }
hmac          = { workspace = true }
sha2          = { workspace = true }

# Observability
tracing       = { workspace = true }
tracing-subscriber = { workspace = true }
anyhow        = { workspace = true }
snafu         = { workspace = true }
```

Note: `unblock-core`, `unblock-github`, `unblock-tools` are **not** dependencies. The agent
is a client of `unblock-mcp-remote` via HTTP. It does not embed domain logic.

### 7.3 Updated workspace `Cargo.toml`

```toml
[workspace]
members = [
    "crates/unblock-core",
    "crates/unblock-github",
    "crates/unblock-tools",
    "crates/unblock-mcp",
    "crates/unblock-mcp-remote",
    "crates/unblock-agent",     # NEW
    "crates/unblock-app",
]
```

### 7.4 Crate dependency graph

```
unblock-agent (bin)
  ├── axum + tower-http
  ├── rig-core              ← LLM loop, does NOT depend on unblock-*
  └── reqwest               ← GitHub Contents API, PR API (raw HTTP)

unblock-mcp-remote (bin)   ← agent calls this via HTTP, not Rust dependency
  └── unblock-tools (lib)
        ├── unblock-github
        └── unblock-core
```

The agent crate is architecturally isolated from the domain layer. This is intentional —
the agent is a consumer of the public MCP interface, exactly as Claude Code and Copilot are.

---

## 8. Auth Model

### 8.1 Service Account Token

The agent service uses a **dedicated GitHub token** separate from any developer's personal
token. Two options:

| Option | Mechanism | Rate limit | Setup complexity |
|---|---|---|---|
| **GitHub App installation token** | GitHub App created for Unblock, installed per repo | 15,000 req/hour per installation | Medium — requires App registration |
| **PAT (fine-grained)** | Personal Access Token scoped to the agent's bot account | 5,000 req/hour | Low — create bot account + PAT |

**Decision: GitHub App for production, fine-grained PAT for early development.**

A GitHub App gives the agent a proper bot identity on GitHub (`unblock-agent[bot]`) — its
comments are visually distinct from human comments. The installation token is short-lived
(1 hour, auto-refreshed). Fine-grained PATs are adequate for initial development and
small teams.

### 8.2 Token Scope (fine-grained PAT)

```
Repository permissions:
  - Issues: Read & Write          (show, comment, label management)
  - Pull requests: Read & Write   (read diff, submit PR review)
  - Contents: Read                (file tree, file content)
  - Metadata: Read                (repo info)
```

No `code` write access. The agent cannot push to branches. This is enforced by the token
scope — not just by convention.

### 8.3 Webhook Verification

Webhooks are verified via HMAC-SHA256 using the `WEBHOOK_SECRET` env var — identical to
the pattern in `unblock-mcp-remote`. The agent webhook handler shares the same
`verify_hmac_sha256` function signature (copy, not shared library — the agent crate does
not depend on `unblock-mcp-remote`).

### 8.4 MCP Authentication

The agent authenticates to `unblock-mcp-remote` with the service account token via the
standard `Authorization: Bearer` header defined in the remote MCP research (§5.1). The
remote MCP validates the token against GitHub's `/user` endpoint (once per session, cached
for 5 minutes). The agent's token fingerprint gets its own `CacheKey` in the
`SharedGraphCache` — independent from any developer's cache entry.

---

## 9. Filesystem Access

The agent runs on a server — it has no access to the developer's local filesystem or git
repository. Code access goes through two channels:

### 9.1 GitHub Contents API — for individual files

```rust
// crates/unblock-agent/src/tools/github.rs

pub async fn fetch_file(
    client: &reqwest::Client,
    token:  &SecretString,
    repo:   &str,
    path:   &str,
    sha:    &str,   // commit SHA or branch name
) -> Result<String, AgentError> {
    // GET /repos/{owner}/{repo}/contents/{path}?ref={sha}
    // Response: base64-encoded content
    let url = format!(
        "https://api.github.com/repos/{repo}/contents/{path}?ref={sha}"
    );
    let resp: ContentResponse = client
        .get(&url)
        .bearer_auth(token.expose_secret())
        .header("Accept", "application/vnd.github.v3+json")
        .send().await?
        .json().await?;

    let bytes = BASE64.decode(resp.content.replace('\n', ""))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
```

Rate limit: 5,000 requests/hour (PAT) or 15,000/hour (GitHub App). At max 12 files per
agent run, each run costs 12 GitHub API calls. At 1,000 runs/day: 12,000 calls/day —
well within limits.

### 9.2 File Discovery Strategy

The agent discovers relevant files in two passes:

**Pass 1 — Directory tree:**
```
GET /repos/{r}/git/trees/{sha}?recursive=1
```
Returns the full file tree (path only, no content). The LLM reads the tree and identifies
which paths are likely relevant based on the issue description. This is a single API call
regardless of repo size — the tree is a flat list of paths.

**Pass 2 — Targeted fetch:**
The LLM identifies at most `max_files` (default 12) paths. The agent fetches them in
parallel. Files over `max_lines_per_file` (default 400) are truncated at that line with
a `[... truncated at 400 lines ...]` marker.

**Why not shallow clone:**
A shallow clone (`git clone --depth 1 --filter=blob:none`) would give full filesystem
access without API rate limit concerns. The tradeoffs:

| Approach | Pro | Contra |
|---|---|---|
| GitHub Contents API | No git tooling required, no disk I/O, parallel fetches | Rate limit per file, no grep across full repo |
| Shallow clone | Full filesystem, grep possible, no rate limit overhead | Requires `git` binary, disk space, auth via HTTPS or SSH, ~5-15s clone time for large repos |

**Decision: GitHub Contents API for Phase 4.** The agent's LLM-guided file selection means
full-repo grep is not needed — the model identifies relevant paths from the tree listing
and issue context. If investigation quality is demonstrably limited by file access patterns,
shallow clone can be evaluated in a follow-up iteration.

### 9.3 PR Diff Access

For the review flow, the diff is fetched directly from the GitHub REST API:

```rust
// GET /repos/{r}/pulls/{pr}/files
// Returns: [{filename, patch, additions, deletions, status}, ...]
pub async fn fetch_pr_diff(
    client:   &reqwest::Client,
    token:    &SecretString,
    repo:     &str,
    pr:       u64,
) -> Result<Vec<PrFileDiff>, AgentError> { ... }
```

This returns per-file unified diffs. No git clone needed. The LLM receives the diff
alongside the issue body and comment trail.

---

## 10. Prompt Design

### 10.1 Principles

Prompts are compiled into the binary as `const &str` — not loaded from files at runtime.
This eliminates a class of runtime failures (missing prompt file, corrupted template) and
ensures prompt versions are tied to binary versions. Changes to prompts require a binary
rebuild and release.

Both prompts share three structural requirements enforced in the system prompt:
1. The output format is prescribed exactly — deviation is treated as a failure by the
   downstream pipeline parser.
2. The agent must stop after posting the structured comment. No follow-up actions.
3. The agent must check idempotency before acting (reinforces the idempotency check in
   the wrapper, as a second layer of defence).

### 10.2 `SHERLOCK_SYSTEM_PROMPT`

```
You are Sherlock, an investigation agent for the ://unblock dependency-aware task system.

## Identity
You are a senior Rust engineer performing pre-implementation investigation. Your role is
to front-load the cognitive work before any code is written. You do not implement. You do
not create branches. You investigate and document.

## Process
1. Call `show` with `include_comments: true`. Read the issue body and all comments.
2. If you find a comment starting with "INVESTIGATION:", stop immediately and reply:
   "INVESTIGATION comment already exists. Nothing to do."
3. Parse the three body sections: Description, Design Notes, Acceptance Criteria.
4. If this is a Sub-Issue, read the parent issue for epic context.
5. Review the file tree and file contents provided to you. Identify relevant code paths.
6. Identify: root cause, relevant files with line numbers, implementation approach,
   risks, related tests, and gaps or ambiguities in the acceptance criteria.
7. Call `comment` with the exact INVESTIGATION format below. Do not deviate.
8. After calling `comment`, stop. Your turn is complete.

## INVESTIGATION Comment Format

Post exactly this structure. Do not add sections. Do not omit sections.

INVESTIGATION:

Root cause: {one or two sentences explaining what the issue requires and why it is
non-trivial — not a restatement of the title}

Files:
- {path}:{line} — {what is relevant at this location}
- {path}:{line} — {what is relevant at this location}
[3-8 entries; quality over quantity]

Approach:
1. {concrete step with file and function name where possible}
2. {concrete step}
[3-6 steps]

Risks:
- {specific risk with file/line reference where applicable}
[1-4 risks; omit section if none]

Related tests:
- {test_file}::{test_name}
[1-4 tests; omit section if none discovered]

Gaps in acceptance criteria:
- {specific ambiguity or missing specification in the acceptance criteria}
[omit section entirely if criteria are complete and unambiguous]

## Boundaries
- You may call: `show`, `comment`
- You may NOT call: `claim`, `close`, `create`, `update`, `depends`, or any write tool
- You may NOT create branches, push code, or open pull requests
- You may NOT implement the feature — investigation only
```

### 10.3 `LINUS_SYSTEM_PROMPT`

```
You are Linus, a code review agent for the ://unblock dependency-aware task system.

## Identity
You are a senior Rust engineer performing structured code review. Your role is to validate
that an implementation satisfies the acceptance criteria of the linked GitHub Issue, using
the comment trail as the implementation audit log. You do not fix code. You do not push
commits. You review and document findings.

## Process
1. Call `show` with `include_comments: true`. Read the issue body and all comments.
2. If you find a comment starting with "REVIEW:", stop immediately and reply:
   "REVIEW comment already exists. Nothing to do."
3. If no "COMPLETED:" comment exists, stop and post:
   "Cannot review: no COMPLETED comment found on issue #{N}. The implementation may
   not be finished. Re-trigger review after the implementing agent posts COMPLETED."
4. Read the diff provided. It is the output of `GET /pulls/{pr}/files`.
5. Cross-reference the diff against the acceptance criteria in the issue body.
6. Note any DECISION or DEVIATION comments — these are explanations from the implementing
   agent for divergences from the spec. Factor them into your verdict.
7. Call `comment` on the Issue with the REVIEW format below.
8. After calling `comment`, stop. Your turn is complete.

## Verdict options

APPROVE      — all acceptance criteria met or acceptably covered by DECISION/DEVIATION
NEEDS-REWORK — one or more acceptance criteria unmet without justification

## Finding severity levels

[CRITICAL] — criterion unmet, no DECISION/DEVIATION justification, cannot approve
[WARNING]  — criterion partially met or implementation choice needs documentation
[INFO]     — observation, suggestion, or question; does not affect verdict

## REVIEW Comment Format

Post exactly this structure. Do not deviate.

REVIEW:

Verdict: {APPROVE | NEEDS-REWORK}

Summary: {2-3 sentences describing the overall implementation quality and any notable
findings}

Criteria check:
- [{GOOD|WARNING|CRITICAL}] Criterion {N}: {assessment}
[one line per acceptance criterion]

Findings:
- [{CRITICAL|WARNING|INFO}] {file}:{line} — {finding description}
[omit section entirely if no findings]

## Boundaries
- You may call: `show`, `comment`
- You may NOT call: `claim`, `close`, `create`, `update`, `depends`, or any write tool
- You may NOT push code, create branches, or merge pull requests
- You may NOT refactor or fix the implementation — review only
```

---

## 11. Output Format Enforcement

The structured comment formats (`INVESTIGATION:`, `REVIEW:`) are consumed downstream by
`/start-task` (skips investigation if comment exists), `/rework-task` (reads REVIEW
findings), and the reconciliation engine. Format drift breaks the pipeline.

Three enforcement layers:

**Layer 1 — System prompt.** The exact format is prescribed in the system prompt. The LLM
is instructed not to deviate.

**Layer 2 — Output validation.** After the agent run, the agent service validates the
comment body before posting it:

```rust
// crates/unblock-agent/src/idempotency.rs

pub fn validate_investigation_comment(body: &str) -> Result<(), FormatError> {
    if !body.starts_with("INVESTIGATION:") {
        return Err(FormatError::MissingHeader("INVESTIGATION:"));
    }
    if !body.contains("\nRoot cause:") {
        return Err(FormatError::MissingSection("Root cause"));
    }
    if !body.contains("\nFiles:") {
        return Err(FormatError::MissingSection("Files"));
    }
    if !body.contains("\nApproach:") {
        return Err(FormatError::MissingSection("Approach"));
    }
    Ok(())
}
```

If validation fails, the agent posts a degraded comment:

```
INVESTIGATION: [FORMAT ERROR]

The investigation agent produced output that did not match the required format.
Raw output preserved below for manual review.

---
{raw LLM output}
```

The pipeline parser sees `INVESTIGATION:` at the start and treats it as existing — the
issue will not be re-investigated automatically. A human can re-trigger by removing and
re-adding the label.

**Layer 3 — Idempotency check.** Before posting, the agent checks whether the comment
already exists. If it does (regardless of format), it does not overwrite.

---

## 12. Webhook Routing

### 12.1 Webhook handler

```rust
// crates/unblock-agent/src/webhook.rs

pub async fn github_webhook(
    State(state): State<AppState>,
    headers:      HeaderMap,
    body:         Bytes,
) -> Result<StatusCode, AppError> {
    verify_hmac_sha256(&headers, &body, &state.webhook_secret)?;

    let event_type = headers
        .get("X-GitHub-Event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let payload: serde_json::Value = serde_json::from_slice(&body)?;

    match event_type {
        "issues" => handle_issues_event(&state, payload).await,
        "pull_request" => handle_pr_event(&state, payload).await,
        _ => Ok(StatusCode::NO_CONTENT), // ignore all other events
    }
}

async fn handle_issues_event(
    state:   &AppState,
    payload: serde_json::Value,
) -> Result<StatusCode, AppError> {
    let action = payload["action"].as_str().unwrap_or("");
    let label  = payload["label"]["name"].as_str().unwrap_or("");

    if action == "labeled" && label == "needs-investigation" {
        let issue  = payload["issue"]["number"].as_u64().unwrap();
        let repo   = payload["repository"]["full_name"].as_str().unwrap();
        let sha    = payload["issue"]["head"]["sha"]    // default branch sha
                         .as_str()
                         .unwrap_or("HEAD");

        // Spawn — webhook handler returns 204 immediately
        tokio::spawn(run_sherlock(state.clone(), repo.to_string(), issue, sha.to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}

async fn handle_pr_event(
    state:   &AppState,
    payload: serde_json::Value,
) -> Result<StatusCode, AppError> {
    let action  = payload["action"].as_str().unwrap_or("");
    let is_draft = payload["pull_request"]["draft"].as_bool().unwrap_or(true);

    if (action == "opened" || action == "ready_for_review") && !is_draft {
        let pr   = payload["pull_request"]["number"].as_u64().unwrap();
        let repo = payload["repository"]["full_name"].as_str().unwrap();
        let sha  = payload["pull_request"]["head"]["sha"].as_str().unwrap();

        tokio::spawn(run_linus(state.clone(), repo.to_string(), pr, sha.to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}
```

**Important:** The webhook handler returns `204 No Content` immediately. Agent runs are
spawned as background tasks. GitHub expects a webhook response within 10 seconds — agent
runs take 30-120 seconds. The spawn pattern ensures the webhook delivery never times out.

### 12.2 Webhook configuration

Two webhook subscriptions on the GitHub repository (or GitHub App):

| Subscription | Events | URL |
|---|---|---|
| Cache invalidation | issues (all) | `https://unblock.example.com/webhooks/github` (remote MCP) |
| Agent dispatch | issues, pull_request | `https://agent.example.com/webhooks/github` (agent service) |

Or if co-deployed on the same domain with path routing:

```
POST /webhooks/github           → unblock-mcp-remote (cache invalidation)
POST /agent/webhooks/github     → unblock-agent (dispatch)
```

---

## 13. Idempotency

Every agent run is idempotent by design. The guard is applied at two levels:

**Level 1 — Webhook handler.** Before spawning the agent task, check the issue/PR state
via a quick `show` call. If the expected comment already exists, return early.

**Level 2 — Agent loop.** The system prompt instructs the agent to call `show
--include_comments` first and abort if the comment is already present.

**Re-trigger mechanism:** If an investigation or review fails or produces a malformed
comment, the human can re-trigger by:
- **Investigation:** Remove and re-add the `needs-investigation` label.
- **Review:** Close and reopen the PR (triggers `pull_request.reopened` — add this to the
  webhook subscription and handle it identically to `opened`).

**Label cleanup:** After a successful investigation run, the `needs-investigation` label is
removed by the agent via the GitHub REST API. This prevents the agent from re-running if
the issue is edited and the webhook fires again.

---

## 14. Quality Considerations

### 14.1 What Codestral does well

- Identifies relevant Rust files from a directory tree — strong code understanding
- Produces structured output reliably when prompted with exact format specifications
- Finds file/line references for specific patterns, functions, and trait implementations
- Detects acceptance criteria gaps — missing error cases, unspecified behaviour

### 14.2 Where frontier models (Claude, GPT-4) still lead

- **Architectural reasoning:** "This abstraction will create debt in 6 months because..."
- **Subtle Rust issues:** Lifetime edge cases, subtle API misuse, concurrency footguns
- **Cross-cutting concerns:** Security implications, performance characteristics, API
  design critique beyond the stated criteria

For the target use case — front-loading investigation and catching mechanical review issues
before `/start-task` — Codestral's quality is adequate. The agent is not a replacement for
the Linus review that runs in the human Claude Code session pipeline. It is a **first pass**
that catches obvious issues and documents context, reducing the cognitive load on the
implementing agent.

### 14.3 Quality gates

| Metric | Acceptable threshold | Measurement |
|---|---|---|
| `INVESTIGATION:` format compliance | > 95% of runs | Validation in `idempotency.rs` |
| Relevant files identified (top 3 in actual diff) | > 80% of runs | Manual audit of 50 runs |
| `REVIEW:` verdict alignment with human review | > 75% match | Manual comparison |
| False APPROVE rate (misses a CRITICAL) | < 5% | Manual audit |

If metrics fall below threshold, the remediation path is:
1. Improve system prompt specificity
2. Evaluate Mistral Small 3.1 as replacement (stronger instruction following)
3. Evaluate fine-tuning on Unblock comment history (Phase 5)

---

## 15. Cost Model

### 15.1 Per-run token estimate

**Sherlock (Investigation):**

| Component | Tokens (estimate) |
|---|---|
| System prompt | 800 |
| Issue body + comment trail | 1,500 |
| File tree (paths only) | 2,000 |
| File contents (12 files × 400 lines avg) | 30,000 |
| LLM multi-turn reasoning | 5,000 |
| Output (INVESTIGATION comment) | 700 |
| **Total** | **~40,000 tokens** |

At Codestral pricing (€0.03/1M input, €0.10/1M output):
~€0.001 per investigation run.

**Linus (PR Review):**

| Component | Tokens (estimate) |
|---|---|
| System prompt | 1,000 |
| Issue body + full comment trail | 3,000 |
| PR diff (medium PR, 10 files) | 8,000 |
| LLM multi-turn reasoning | 3,000 |
| Output (REVIEW comment) | 800 |
| **Total** | **~16,000 tokens** |

~€0.0005 per review run.

### 15.2 Monthly cost at scale

| Volume | Investigation runs/month | Review runs/month | Mistral API cost |
|---|---|---|---|
| Small team (5 devs) | 100 | 150 | ~€0.18 |
| Medium team (20 devs) | 500 | 750 | ~€0.88 |
| Large team (100 devs) | 3,000 | 5,000 | ~€5.50 |

The token cost is negligible. The dominant cost at scale is infrastructure (Fly.io instance
for the agent service + `unblock-mcp-remote`).

### 15.3 Infrastructure cost

| Deployment | Monthly cost |
|---|---|
| Fly.io shared-cpu-1x (256MB) — agent service | ~$5 |
| Fly.io shared-cpu-1x (512MB) — remote MCP | ~$7 |
| **Total infra** | **~$12/month** |

No GPU required for Mistral API usage — inference runs on Mistral's infrastructure.
GPU cost only arises if self-hosting (Phase 5 decision gate).

---

## 16. Deployment

### 16.1 Docker image

```dockerfile
FROM rust:1.82-slim AS builder
WORKDIR /app
COPY . .
RUN cargo build --release -p unblock-agent

FROM debian:bookworm-slim
RUN apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/unblock-agent /usr/local/bin/
EXPOSE 3001
CMD ["unblock-agent"]
```

### 16.2 Release workflow

```yaml
# .github/workflows/agent-release.yml
on:
  push:
    tags: ['unblock-agent-v*']

jobs:
  docker:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: docker/build-push-action@v5
        with:
          push: true
          tags: ghcr.io/websublime/unblock-agent:${{ github.ref_name }}
```

### 16.3 Environment variables

| Variable | Required | Default | Notes |
|---|---|---|---|
| `MISTRAL_API_KEY` | Yes | — | Mistral API authentication |
| `MISTRAL_MODEL` | No | `codestral-latest` | Override for testing with other models |
| `GITHUB_TOKEN` | Yes | — | Service account PAT or GitHub App installation token |
| `WEBHOOK_SECRET` | Yes | — | HMAC-SHA256 secret for GitHub webhook verification |
| `UNBLOCK_REMOTE_URL` | Yes | — | `https://unblock.example.com/mcp` — remote MCP endpoint |
| `BIND_ADDR` | No | `0.0.0.0:3001` | HTTP server bind address |
| `AGENT_MAX_TURNS` | No | `15` | Safety limit for LLM turns per run |
| `AGENT_MAX_FILES` | No | `12` | Max files fetched per investigation |
| `AGENT_MAX_LINES` | No | `400` | Max lines per file before truncation |
| `AGENT_TIMEOUT_SECS` | No | `120` | Timeout per agent run |
| `UNBLOCK_LOG_LEVEL` | No | `info` | Log level |
| `UNBLOCK_OTEL_ENDPOINT` | No | — | OpenTelemetry collector |

### 16.4 CI/CD impact on existing GitHub Actions

With both `unblock-mcp-remote` and `unblock-agent` deployed, the existing
`unblock-review.yml` and `unblock-qa.yml` Actions are unchanged. The agent flows are
additive. The GitHub Actions review (Linus via Claude Code) still runs on the
`needs-review` label. The agent review (Linus via Codestral) runs on `pull_request.opened`.

In practice this means PRs get **two** reviews: the Codestral fast review (minutes after
PR opens) and the Claude Code review (triggered by the pipeline label). The Claude review
is the canonical gating review. The Codestral review is a fast first pass.

If the two-review model creates noise, the GitHub Actions review can be gated to skip if
a `REVIEW:` comment from the agent already exists.

---

## 17. GHE Support

The agent service follows the same pattern as `unblock-mcp-remote`:

- `GITHUB_API_URL` controls the GitHub API base URL for all outbound calls
- `GITHUB_URL` controls the web URL for display purposes
- No per-request URL override (SSRF prevention)
- `UNBLOCK_REMOTE_URL` points to the GHE-internal `unblock-mcp-remote` instance

GHE deployment scenario:

```bash
docker run \
  -e MISTRAL_API_KEY=xxx \
  -e GITHUB_TOKEN=ghe_xxx \
  -e WEBHOOK_SECRET=xxx \
  -e UNBLOCK_REMOTE_URL=https://unblock.internal.corp.com/mcp \
  -e GITHUB_API_URL=https://ghe.corp.com/api/v3 \
  -e GITHUB_URL=https://ghe.corp.com \
  -p 3001:3001 \
  ghcr.io/websublime/unblock-agent
```

For GHE environments with data residency requirements, the Mistral API call can be replaced
with a self-hosted vLLM endpoint — change `MISTRAL_API_KEY` and point `MISTRAL_BASE_URL`
to the internal vLLM server. The agent code is unchanged.

---

## 18. Licensing Implications

The `unblock-agent` crate ships as a separate binary. It is part of the Pro/Enterprise
offering — not the open-core MIT layer. Licensing follows the same pattern as
`unblock-app` and `unblock-mcp-remote`:

| Crate | License | Distribution |
|---|---|---|
| `unblock-core` | MIT | Open source, always |
| `unblock-github` | MIT | Open source, always |
| `unblock-tools` | MIT | Open source, always |
| `unblock-mcp` | MIT | Open source, always |
| `unblock-mcp-remote` | BSL 1.1 | Pro/Enterprise |
| `unblock-agent` | BSL 1.1 | Pro/Enterprise |
| `unblock-app` | BSL 1.1 (classified) | Enterprise |

The BSL 1.1 converts to MIT after 4 years. Production use of `unblock-mcp-remote` and
`unblock-agent` requires a Pro or Enterprise subscription. Self-hosted GHE deployments
fall under the Enterprise tier.

---

## 19. Alternatives Evaluated

### 19.1 Using Claude via Anthropic API instead of Mistral

| Factor | Claude API | Mistral (Codestral) |
|---|---|---|
| Code quality | Superior | Adequate for the task |
| Data sovereignty | Data leaves to Anthropic | Data leaves to Mistral (or stays on-prem) |
| Cost | €3–€15/1M tokens | €0.03–€0.10/1M tokens |
| Enterprise compliance | Anthropic DPA required | Mistral DPA or self-hosted |
| Vendor lock-in | High | Low (OpenAI-compatible interface) |
| Fine-tuning | Not available | Available (Mistral fine-tuning API) |

Claude is the right model for interactive developer sessions (Claude Code). For a
background agent that runs autonomously on every issue and PR, Mistral/Codestral is the
better fit economically and strategically.

### 19.2 Using Copilot Coding Agent instead of custom agent

The Copilot Coding Agent is designed for the full implementation cycle — investigation →
implement → PR. It cannot be scoped to investigation-only or review-only flows without
breaking its product model. Its output format is prose for human consumption, not the
structured `INVESTIGATION:` / `REVIEW:` comment syntax that the Unblock pipeline parses.
Additionally, Copilot Coding Agent runs on GitHub's infrastructure — no data sovereignty
option.

### 19.3 Using GitHub Actions + Claude Code headless instead of dedicated service

The existing review/QA Actions already do this — Claude Code runs headless in CI. The
problem for investigation-on-issue-creation is latency and resource usage: a full
GitHub Actions runner spin-up takes 30-60 seconds and consumes Actions minutes. An
always-warm `unblock-agent` service running on Fly.io starts the agent loop in <1 second
and has no Actions minutes cost. For high-frequency events (many issues labeled per day),
the always-warm service is significantly cheaper.

### 19.4 Embedding `unblock-tools` in the agent (no remote MCP)

Direct dependency on `unblock-tools` would eliminate the HTTP round-trip per tool call.
Rejected because: the `SharedGraphCache` in `unblock-mcp-remote` is the authoritative
warm cache; bypassing it means a cold graph rebuild per agent run. The HTTP overhead
(<1ms on the same network) is a better trade-off than cold start on every agent execution.

---

## 20. What Does Not Change

The agent is additive. No existing component is modified.

| Component | Change |
|---|---|
| `unblock-core` | Zero |
| `unblock-github` | Zero |
| `unblock-tools` | Zero |
| `unblock-mcp` (stdio) | Zero |
| `unblock-mcp-remote` | Zero — agent is a client, not a modification |
| Plugin agents (Sherlock, Linus `.md` files) | Zero — these continue to run in CC sessions |
| `unblock-review.yml` / `unblock-qa.yml` | Zero — still run on label triggers |
| GitHub as source of truth | Preserved — agent writes comments, reads via MCP + Contents API |
| Stateless by design | Preserved — agent runs are ephemeral per event |

The existing Sherlock in the plugin and the Sherlock in the agent service are independent.
The plugin Sherlock runs inside a developer's Claude Code session when `/start-task` is
invoked. The agent Sherlock runs autonomously on `needs-investigation` label. Both write
to the same `INVESTIGATION:` comment format — the pipeline is agnostic to which one ran.

---

## 21. Open Questions

| Question | Notes |
|---|---|
| GitHub App vs fine-grained PAT for initial development | App gives bot identity (`unblock-agent[bot]`) which is valuable for comment attribution. PAT is simpler for Phase 4 start. Recommend: PAT for Phase 4.0, App for Phase 4.1 |
| Should the agent post inline PR review comments or only the REVIEW: comment? | Inline comments (file:line) are more useful for the developer but require the GitHub PR review API. The REVIEW: comment on the Issue is sufficient for pipeline consumption. Recommend: REVIEW: comment on Issue in Phase 4; inline comments as follow-up |
| Two-review scenario: Codestral fast review + Claude Code gating review — noise? | The Codestral review arrives fast and may conflict with Claude's. Mitigation: the agent uses `event: "COMMENT"` in the PR review API (never APPROVE or REQUEST_CHANGES). The Claude Code review remains the canonical gate |
| `rig` crate maturity | `rig` is young (0.1.x). If tool calling reliability is insufficient, the fallback is a hand-rolled agent loop using `reqwest` + Mistral chat completions API directly. The architecture does not change — only the agent loop implementation |
| Fine-tuning data collection | INVESTIGATION and REVIEW comments accumulate in GitHub Issues. After 500+ issues, a Mistral fine-tuning job on this data could significantly improve format compliance and investigation quality. Needs explicit consent mechanism if user data is involved |
| Rate limiting the agent service | A single malicious webhook replay could trigger many agent runs. The webhook handler should include a per-repo rate limit (e.g., max 10 agent runs per repo per hour) using an in-memory counter |
| Should the agent remove the `needs-investigation` label on success? | Yes — prevents re-trigger on issue edits. On failure (format error, timeout), keep the label so the human knows the run failed. The degraded comment (§11) signals failure |
| `UNBLOCK_REMOTE_URL` for multi-repo configurations | A single agent service deployment can serve multiple repos. The `UNBLOCK_REMOTE_URL` is global — all repos use the same remote MCP. This is correct if the remote MCP is also serving all repos (which it is, keyed by repo in `SharedGraphCache`) |
