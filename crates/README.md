# crates — Rust workspace for `unblock-code` AST CLI

This directory will host the Rust workspace that produces `unblock-code`: a
local, stateless, one-shot CLI binary that indexes source code into a SQLite
+ FTS5 database and exposes structured queries (find-symbol, outline, search,
…) over a JSON envelope. Designed to save AI-agent tokens that would otherwise
go to `Glob`/`Grep`/`Read`.

## Status

**Empty skeleton.** Bootstrap deferred until Stage 3 of the new product
roadmap. The full design is preserved in [`docs/code-cli/`](../docs/code-cli/) —
plan, spec, and research from the original Phase 03 design carry forward
verbatim.

## Planned crates

- `unblock-indexer-core` — pure types, AST traversal, schema constants (zero IO)
- `unblock-indexer` — sqlx + FTS5, statically-linked tree-sitter grammars,
  filesystem walker
- `unblock-code` — clap-based CLI binary (one-shot, stateless)

## Key constraints carried forward

- **Standalone**. The CLI is fully decoupled from the Encore backend. It does
  not consume the API. `unblock-code` and the issue-tracker `mcp` service share
  zero runtime state. This is locked in `docs/code-cli/spec.md` §3 explicit
  non-goal.
- 10 statically-linked tree-sitter grammars: 8 default
  (Rust, TypeScript, JavaScript, Python, Go, Java, C, PHP) + 2 opt-in
  (`lang-cpp`, `lang-ruby`).
- 17 canonical `SymbolKind` variants.
- 11 commands.
- Local SQLite + FTS5 + WAL index at `~/.cache/unblock/repos/<repo-hash>/index.db`.
- One-shot stateless invocation — no daemon, no watcher, no MCP.

## Distribution (planned)

- `cargo install unblock-code`
- Homebrew tap: `brew install websublime/tap/unblock-code`
- npm wrapper: `npx @unblock/code`

See `docs/code-cli/plan.md` for the epic structure and acceptance criteria.
