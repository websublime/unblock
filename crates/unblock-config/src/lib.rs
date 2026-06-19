//! `unblock-config` (L4) — layered TOML config resolution (CLI > env `UNBLOCK_*` > project
//! `.unblock/config.toml` > defaults), `.unblock/` discovery, and the workspace-open facade
//! (CF-D): it opens/migrates libsql and builds the `Arc<dyn Storage>` carried by
//! `WorkspaceContext`; the engine consumes the context, it does not construct storage.
//! See `docs/plans/crates/unblock-config.md`.
#![forbid(unsafe_code)]
