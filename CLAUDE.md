# ://unblock

Dependency-aware task tracking for AI agents, powered by GitHub.

## Project Overview

MCP server that turns GitHub Issues into a dependency graph. Agents ask `ready` and get unblocked work. GitHub stores, Rust computes.

## Tech Stack

- Language: Rust (edition 2024)
- Workspace: `crates/unblock-core`, `crates/unblock-github`, `crates/unblock-mcp`
- Graph engine: petgraph
- MCP protocol: rmcp
- HTTP client: reqwest
- Error handling: snafu
- Logging: tracing
- Async runtime: tokio

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
- **unblock-mcp** — MCP server binary. 17 tool handlers, stdio transport

## Commit Strategy

- Conventional commits: `feat:`, `fix:`, `docs:`, `chore:`, `refactor:`, `test:`
- Atomic commits — each commit compiles and passes tests
- Fix code, not tests

## Documentation

- `docs/unblock-prd-github.md` — what to build (MCP)
- `docs/unblock-architecture-github.md` — how to build it (MCP)
- `docs/unblock-project-plan.md` — when to build it (MCP)
- `docs/unblock-cicd-architecture.md` — how to ship it
- `docs/unblock-prd-plugin.md` — what to build (plugin)
- `docs/unblock-architecture-plugin.md` — how to build it (plugin)
- `DEV-ROADMAP.md` — development roadmap
