//! Pure routing: `Command` → the matching `commands::*::run`, plus the shared `SessionConfig`
//! assembly for the storage-opening commands (`mcp`/`migrate`/`doctor`).
//!
//! **Who opens a `WorkspaceContext` via `open_with_storage_with_cli`:** mcp, migrate, doctor.
//! **Who does NOT open storage:** `version` (pure `build.rs` env — runs OUTSIDE a workspace),
//! `update` (self-update, no workspace). **`init`** creates + opens through the facade (one code path,
//! FR-9 no-drift), and `init --agents` then runs the shared `AGENTS.md` write at that open's own
//! `workspace_dir`. **`agents`** opens resolve-only (NO DB) to learn `workspace_dir`, then runs the
//! same write.
//!
//! Each handler returns `Result<Option<u8>, CliError>`: `Some(128+signo)` = an `mcp` signal exit;
//! `None` = success (exit 0). `run_with` maps that through the exit boundary.

use unblock_engine::SessionConfig;

use crate::cli::{Cli, Command};
use crate::commands;
use crate::exit::CliError;

/// Route a parsed `Cli` to its command handler.
///
/// # Errors
/// Forwards any `CliError` from the command handler (config/engine/mcp/render/local).
pub async fn dispatch(cli: Cli) -> Result<Option<u8>, CliError> {
    let overrides = cli.global.to_overrides();
    match cli.command {
        // No storage / no workspace.
        Command::Version(args) => commands::version::run(&args, &cli.global),
        // File ops (resolve-only or scaffold-then-open through the facade).
        Command::Init(args) => commands::init::run(&args, &cli.global).await,
        Command::Agents(args) => commands::agents::run(&args, &cli.global).await,
        // Storage-opening commands.
        Command::Mcp(args) => commands::mcp::run(&args, &overrides).await,
        Command::Migrate(args) => commands::migrate::run(&args, &overrides).await,
        Command::Doctor(args) => commands::doctor::run(&args, &overrides).await,
        #[cfg(feature = "self-update")]
        Command::Update(args) => commands::update::run(&args).await,
    }
}

/// Assemble the `SessionConfig` the storage-opening commands pass to `Session::open`.
///
/// **`import_on_open` MUST stay false in v1** — `true` returns `FeatureNotWired{"sync"}` (exit 1),
/// the DR-13 trap. `remote` is `false` (v1). (D57 deleted the `jsonl_export` copy: the engine reads
/// the live runtime snapshot, so a copy here would go stale on a reload.)
#[must_use]
pub fn session_config() -> SessionConfig {
    SessionConfig {
        import_on_open: false,
        remote: false,
    }
}

#[cfg(test)]
mod tests {
    use super::session_config;

    #[test]
    fn session_config_never_imports_on_open() {
        let cfg = session_config();
        assert!(!cfg.import_on_open, "DR-13 trap: must stay false");
        assert!(!cfg.remote, "v1: remote stays false");
    }
}
