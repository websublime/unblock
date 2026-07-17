//! Pure routing: `Command` → the matching `commands::*::run`, plus the shared `SessionConfig`
//! assembly for the storage-opening commands (`mcp`/`migrate`/`doctor`).
//!
//! **Who opens a `WorkspaceContext` via `open_with_storage_with_cli`:** mcp, migrate, doctor.
//! **Who does NOT open storage:** `version` (pure `build.rs` env — runs OUTSIDE a workspace),
//! `update` (self-update, no workspace). **`init`** creates + opens through the facade (one code path,
//! FR-9 no-drift). **`agents`** opens resolve-only (NO DB) to learn `workspace_dir`.
//!
//! Each handler returns `Result<Option<u8>, CliError>`: `Some(128+signo)` = an `mcp` signal exit;
//! `None` = success (exit 0). `run_with` maps that through the exit boundary.

use unblock_config::WorkspaceContext;
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
/// the DR-13 trap. `remote` is `false` (v1). `jsonl_export` mirrors the resolved config value.
#[must_use]
pub fn session_config(ctx: &WorkspaceContext) -> SessionConfig {
    SessionConfig {
        jsonl_export: ctx.config.jsonl_export,
        import_on_open: false,
        remote: false,
    }
}

#[cfg(test)]
mod tests {
    use super::session_config;
    use unblock_config::{ConfigPaths, ResolvedConfig, WorkspaceContext, WorkspaceSource};
    use unblock_storage::LibsqlStorage;

    async fn workspace_context(jsonl_export: bool) -> WorkspaceContext {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("unblock.db");
        let storage =
            LibsqlStorage::open_local(&db_path, unblock_storage::DEFAULT_WRITE_LOCK_TIMEOUT_MS)
                .await
                .expect("open db");
        let config = ResolvedConfig {
            jsonl_export,
            ..ResolvedConfig::default()
        };
        WorkspaceContext {
            storage: std::sync::Arc::new(storage),
            workspace_dir: tmp.path().to_path_buf(),
            actor: "tester".to_string(),
            config,
            paths: ConfigPaths {
                unblock_dir: tmp.path().to_path_buf(),
                db_path,
                jsonl_path: tmp.path().join("issues.jsonl"),
            },
            source: WorkspaceSource::WalkUp,
        }
    }

    #[tokio::test]
    async fn session_config_never_imports_on_open_and_mirrors_jsonl() {
        let ctx = workspace_context(true).await;
        let cfg = session_config(&ctx);
        assert!(cfg.jsonl_export, "mirrors ctx.config.jsonl_export");
        assert!(!cfg.import_on_open, "DR-13 trap: must stay false");
        assert!(!cfg.remote, "v1: remote stays false");
    }
}
