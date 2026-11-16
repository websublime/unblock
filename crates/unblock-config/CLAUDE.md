# unblock-config — L4

Layered TOML config (CLI > env `UNBLOCK_*` > project `.unblock/config.toml` > defaults),
`.unblock/` discovery, and the workspace-open facade (CF-D): opens/migrates libsql and builds the
`Arc<dyn Storage>` in `WorkspaceContext`. Remote-mode auth tokens NEVER from `config.toml` (NFR-18).
Every storage-bearing open refuses a LOST database (absent or 0 bytes beside `.write.lock`) with
`ConfigError::DatabaseMissing` unless `CliOverrides::create_db` is set; `open_with_cli` reads the
`no_db` and `storage_open` open modes (D58).

- **Plan (authoritative):** [`docs/plans/crates/unblock-config.md`](../../docs/plans/crates/unblock-config.md)
- **Interface SSOT:** `docs/plans/01-design-spine.md` §4 · **Product:** `docs/PRD.md`
- **Depends on:** `storage`, `sync`, `health`, `model`, `error` (`model` is a DIRECT L0->L4 edge — the `OutputFormat` re-export, G-7/CF-J).
