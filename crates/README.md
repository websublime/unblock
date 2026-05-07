# crates — Rust workspace

This directory hosts the Rust workspace for `://unblock`. It produces **two
distinct binaries** that ship via the same release pipeline (cargo-dist +
Homebrew + npm) but solve unrelated problems:

1. **`unblock-code`** — local, stateless, one-shot AST CLI that indexes
   source code into a SQLite + FTS5 database and exposes structured queries
   (find-symbol, outline, search, …) over a JSON envelope. Designed to save
   AI-agent tokens that would otherwise go to `Glob`/`Grep`/`Read`.
2. **`unblock-plugin`** — mister-anderson workflow renderer that emits typed
   agents, skills, hooks, and MCP config onto Claude Code, GitHub Copilot
   cloud, and GitHub Copilot local from a single Rust catalogue.

## Status

**Empty skeleton.** Bootstrap deferred until Stage 3 of the new product
roadmap. The AST CLI design is preserved verbatim in
[`docs/code-cli/`](../docs/code-cli/) (plan, spec, research). The plugin
renderer surface is specified in `docs/PRD.md` §6 and detailed in
`docs/SPEC.md` (post-Stage-1).

## Planned crates (4)

- **`unblock-indexer-core`** (lib) — pure types, AST traversal, schema
  constants (zero IO, zero async, zero tokio). Owned by the AST CLI.
- **`unblock-indexer`** (lib) — sqlx + FTS5, statically-linked tree-sitter
  grammars, filesystem walker via `ignore`. Owned by the AST CLI.
- **`unblock-code`** (bin) — clap-based AST CLI. Standalone.
- **`unblock-plugin`** (bin) — clap-based plugin renderer. Standalone.

## AST CLI (`unblock-code`) — key constraints

- **Standalone**. Fully decoupled from the Encore backend. Does not consume
  the API. `unblock-code` and the issue-tracker `mcp` service share zero
  runtime state. Locked in `docs/code-cli/spec.md` §3 as an explicit non-goal.
- 10 statically-linked tree-sitter grammars: 8 default
  (Rust, TypeScript, JavaScript, Python, Go, Java, C, PHP) + 2 opt-in
  (`lang-cpp`, `lang-ruby`).
- 17 canonical `SymbolKind` variants.
- 11 commands: find-symbol, list-symbols, outline, get-symbol, search,
  find-references (HEURISTIC), reindex, status, languages, init, parse.
- Local SQLite + FTS5 + WAL index at `~/.cache/unblock/repos/<repo-hash>/index.db`.
- One-shot stateless invocation — no daemon, no watcher, no MCP.

## Plugin renderer (`unblock-plugin`) — key surface

- Renders the mister-anderson workflow (8 fixed personas + dynamic
  supervisors + 20 skills + 3 hooks) onto three editor targets:
  Claude Code, GitHub Copilot cloud, GitHub Copilot local.
- CLI: `unblock-plugin render --target=<t> --supervisors=<list> --out=<dir>`.
- Single typed catalogue in Rust source — emits per-target artefacts:
  `.claude/agents/`, `.claude/skills/`, `.github/agents/`,
  `.claude/hooks/`, `.claude/settings.json`,
  `.github/copilot-instructions.md`.
- Description-contract lint runs at `build.rs` time (every slash-skill's
  description must follow the imperative-verb + trigger-phrase + stage-tag
  contract — see `docs/PRD.md` §6.7).

## Distribution

Both binaries ship via the same pipeline:

- `cargo install unblock-code` / `cargo install unblock-plugin`
- Homebrew tap: `brew install websublime/tap/unblock-code` / `unblock-plugin`
- npm wrapper: `npx @unblock/code` / `npx @unblock/plugin`

See `docs/code-cli/plan.md` for the AST CLI epic structure and acceptance
criteria. The plugin renderer's epic structure lands in
`docs/plans/NN-plan-plugin.md` after Stage 2.
