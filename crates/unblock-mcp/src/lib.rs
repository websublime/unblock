//! `unblock-mcp` (L7) — the PRIMARY product surface: an `rmcp` stdio MCP server (`unblock serve`)
//! exposing the engine as the consolidated 7-tool taxonomy + resources + prompts, schemars-validated
//! under quotas (NFR-18), discoverable via a `contract_version`-stamped bundle (FR-12), every error
//! mapped to the structured boundary (FR-11). A thin adapter over `Session` (no write orchestration).
//! See `docs/plans/crates/unblock-mcp.md`.
#![forbid(unsafe_code)]
