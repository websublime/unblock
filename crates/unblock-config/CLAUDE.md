# unblock-config — L4

Layered TOML config (CLI > env `UNBLOCK_*` > project `.unblock/config.toml` > defaults),
`.unblock/` discovery, and the workspace-open facade (CF-D): opens/migrates libsql and builds the
`Arc<dyn Storage>` in `WorkspaceContext`. libsql auth tokens NEVER from `config.toml` (NFR-18).

- **Plan (authoritative):** [`docs/plans/crates/unblock-config.md`](../../docs/plans/crates/unblock-config.md)
- **Interface SSOT:** `docs/plans/01-design-spine.md` §4 · **Product:** `docs/PRD.md`
- **Depends on:** `storage`, `sync`, `health`, `error`.
