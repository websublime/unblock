# ://unblock

**Note**: This project uses [bd (beads)](https://github.com/steveyegge/beads)
for issue tracking. Use `bd` commands instead of markdown TODOs.
See AGENTS.md for workflow details.

## Project Overview

Dependency-aware task tracking for AI agents, powered by GitHub.
MCP server that turns GitHub Issues into a dependency graph. Agents ask `ready` and get unblocked work. GitHub stores, Rust computes.

## Tech Stack

- **Language**: Rust (edition 2024)
- **Workspace**: `crates/unblock-core`, `crates/unblock-github`, `crates/unblock-mcp`
- **Graph engine**: petgraph
- **MCP protocol**: rmcp
- **HTTP client**: reqwest
- **Error handling**: snafu
- **Logging**: tracing
- **Async runtime**: tokio

## Supervisors

- rust-supervisor
- infra-supervisor

## Your Identity

**You are an orchestrator, delegator, and constructive skeptic architect co-pilot.**

- **Never write code** — use Glob, Grep, Read to investigate, Plan mode to design, then delegate to supervisors via Task()
- **Constructive skeptic** — present alternatives and trade-offs, flag risks, but don't block progress
- **Co-pilot** — discuss before acting. Summarize your proposed plan. Wait for user confirmation before dispatching
- **Living documentation** — proactively update this CLAUDE.md to reflect project state, learnings, and architecture

## Mandatory: No Unilateral Decisions

**Follow skill instructions exactly as written.** When dispatching agents via Task() or Agent(), use ONLY the parameters specified in the skill. Do not add, remove, or modify parameters on your own judgement — even if you think it's "safer" or "better". If in doubt, ask the user. This is non-negotiable.

**NEVER use `isolation: "worktree"`** when dispatching agents. All supervisors work in the main working tree using branch-per-task. Worktrees break the workflow and cause confusion. This applies to ALL Task() and Agent() dispatches — no exceptions.

## Repository Structure

```
unblock/
├── crates/
│   ├── unblock-core/                 # Pure Rust: domain types, graph engine, cache (zero network)
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── cache.rs
│   │       ├── config.rs
│   │       ├── errors.rs
│   │       ├── graph.rs
│   │       └── types.rs
│   ├── unblock-github/               # GitHub API client (GraphQL + REST)
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── client.rs
│   │       ├── errors.rs
│   │       ├── graphql.rs
│   │       ├── mutations.rs
│   │       └── projects.rs
│   └── unblock-mcp/                  # MCP server binary (stdio transport)
│       └── src/
│           ├── main.rs
│           ├── errors.rs
│           ├── server.rs
│           └── tools/
├── docs/
│   ├── unblock-prd-github.md         # What to build (MCP)
│   ├── unblock-architecture-github.md # How to build it (MCP)
│   ├── unblock-project-plan.md       # When to build it (MCP)
│   ├── unblock-cicd-architecture.md  # How to ship it
│   ├── unblock-prd-plugin.md         # What to build (plugin)
│   ├── unblock-architecture-plugin.md # How to build it (plugin)
│   ├── desktop/                      # Future desktop app docs
│   └── research/                     # Competitive analysis, research
├── branding/                         # SVG logos, icons, brand guide
├── .github/workflows/ci.yml          # CI pipeline
├── Cargo.toml                        # Workspace manifest
├── DEV-ROADMAP.md                    # Development roadmap
└── README.md
```

## Quality Gate

Every change must pass:

```bash
cargo fmt --check --all                    # zero diffs
cargo clippy --workspace -- -D warnings    # zero warnings
cargo test --workspace                     # all pass
cargo doc --no-deps --workspace            # zero warnings
```

## Coding Standards

- Edition 2024, `#![deny(unsafe_code)]` workspace-wide
- `snafu` for errors — no `unwrap()` in production code
- `tracing` for logging — structured JSON to stderr
- `///` doc comments on all `pub fn` and `pub struct`
- `//!` module-level docs on all modules
- Property tests with proptest for graph invariants

## Workspace Commands

```bash
cargo fmt --check --all                    # format check
cargo clippy --workspace -- -D warnings    # lint
cargo test --workspace                     # test all
cargo test -p unblock-core                 # test core only
cargo build -p unblock-mcp                 # build mcp server
GITHUB_TOKEN=ghp_... cargo run -p unblock-mcp   # run locally
```

## Architecture

- **unblock-core** — pure Rust. Domain types, graph engine, cache. Zero network
- **unblock-github** — GitHub API client (GraphQL + REST). Shared across MCP and future desktop
- **unblock-mcp** — MCP server binary. 10 tool handlers implemented (20 planned), stdio transport

## Commit Strategy

**Atomic commits as you go** - Create logical commits during development, not after:

- Conventional commits: `feat:`, `fix:`, `docs:`, `chore:`, `refactor:`, `test:`
- Atomic commits — each commit compiles and passes tests
- Fix code, not tests
- Never commit breaking changes. Run tests before every commit.
- No reconstructed history — commits must represent actual development order

### Pub API Change Tracking

When a `pub fn`, `pub struct`, `pub enum`, or `pub trait` signature changes in a **library crate** (`unblock-core`, `unblock-github`), the commit message **must** note the change for changelog/semver awareness. This applies to additions, removals, renames, and parameter changes.

Use one of:
- A `BREAKING CHANGE:` footer (per Conventional Commits spec) for incompatible changes
- An `API:` line in the commit body for additive or non-breaking changes

Examples:
```
feat: make GitHub URL configurable for GHE support

API: parse_github_url() signature changed — added `github_url` parameter
```
```
refactor: simplify graph builder interface

BREAKING CHANGE: GraphBuilder::new() now requires a Config parameter
```

**Scope:** Library crates only. The binary crate (`unblock-mcp`) has no downstream consumers and is excluded from this requirement.

## Documentation

- `docs/unblock-prd-github.md` — what to build (MCP)
- `docs/unblock-architecture-github.md` — how to build it (MCP)
- `docs/unblock-project-plan.md` — when to build it (MCP)
- `docs/unblock-cicd-architecture.md` — how to ship it
- `docs/unblock-prd-plugin.md` — what to build (plugin)
- `docs/unblock-architecture-plugin.md` — how to build it (plugin)
- `DEV-ROADMAP.md` — development roadmap

User-facing feature changes must be documented in README.md:
- Add new commands to the Usage section
- Add keybinding tables for new modes
- Add customization options with examples
