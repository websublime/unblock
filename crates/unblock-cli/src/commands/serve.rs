//! `unblock serve` (FR-20, D27/AD-4) — the PRIMARY runtime command.
//!
//! Opens a `WorkspaceContext`, installs the FR-17 shutdown handle, opens the `Session` wired to the
//! handle's flag, and runs the LIVE 2-arg `unblock_mcp::serve(Arc<Session>, ServeOptions)` (transport
//! internal `stdio()`). On EOF / first signal the `CancellationToken` cancels → `serve` returns `Ok`
//! → `session.shutdown()` (drain the write permit, clean libsql close). Returns `Some(128+signo)` on a
//! signal exit, `None` on a clean EOF exit. stdout carries ONLY MCP framing (logging is stderr-only).

use std::sync::Arc;

use snafu::ResultExt;
use unblock_config::{CliOverrides, open_with_storage_with_cli};
use unblock_engine::Session;
use unblock_mcp::{Quotas, ServeOptions, serve};

use crate::cli::ServeArgs;
use crate::dispatch::session_config;
use crate::exit::{CliError, McpSnafu};
use crate::shutdown;

/// Run `unblock serve`.
///
/// # Errors
/// - [`CliError::Config`] if the workspace cannot be opened (discovery/DB/migrate);
/// - [`CliError::Engine`] if the session cannot be opened or `shutdown()` fails;
/// - [`CliError::Mcp`] if the MCP server fails to start or its run loop ends abnormally.
pub async fn run(_args: &ServeArgs, overrides: &CliOverrides) -> Result<Option<u8>, CliError> {
    let ctx = open_with_storage_with_cli(overrides).await?;
    let cfg = session_config(&ctx);

    // Install the shutdown handle BEFORE opening the session so the flag is wired from the first tick.
    let handle = shutdown::install();

    let session = Session::open(ctx, cfg)
        .await?
        .with_shutdown_flag(handle.flag.clone());
    let session = Arc::new(session);

    let opts = ServeOptions {
        instructions: Some(instructions()),
        quotas: Quotas::default(),
        cancel: handle.token.clone(),
    };

    // Runs until EOF / cancellation. A signal cancels the token (see shutdown.rs), returning Ok here.
    serve(Arc::clone(&session), opts).await.context(McpSnafu)?;

    // Clean cooperative shutdown: drain the in-flight write permit, leave libsql idle for a clean close.
    session.shutdown().await?;

    // `Some(128+signo)` when a signal drove the shutdown; `None` on a clean EOF exit (0).
    Ok(handle.signal_exit_code())
}

/// The optional client-facing MCP instructions string (advertised on `initialize`).
fn instructions() -> String {
    format!(
        "Connect to unblock over MCP stdio (contract {}). Issue-data verbs are MCP tools; the CLI is \
         lifecycle/ops only.",
        unblock_mcp::CONTRACT_VERSION
    )
}

#[cfg(test)]
mod tests {
    use super::instructions;

    #[test]
    fn instructions_mention_the_contract_version() {
        let text = instructions();
        assert!(text.contains(unblock_mcp::CONTRACT_VERSION));
        assert!(text.contains("MCP stdio"));
    }
}
