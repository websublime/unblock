# unblock-mcp — L7 (PRIMARY)

The primary surface: an `rmcp` stdio server (`unblock mcp`) exposing the engine as 7 tools +
resources + prompts, schemars-validated under quotas (NFR-18), `contract_version`-stamped (FR-12).
A **thin adapter** over `Session` — no domain logic, no write orchestration (the engine owns the
write Semaphore, D14). Tool count ≤ 8. No libsql/backend type; no git.

- **Plan (authoritative):** [`docs/plans/crates/unblock-mcp.md`](../../docs/plans/crates/unblock-mcp.md)
- **Interface SSOT:** `docs/plans/01-design-spine.md` §5 · **Product:** `docs/PRD.md`
- **Depends on:** `engine`, `render`, `policy`, `model`, `error`. Never `unblock-cli` (cli → mcp only).
