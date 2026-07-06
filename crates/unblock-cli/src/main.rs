//! `unblock` — process entry point for the lifecycle/ops CLI. Builds the tokio runtime and delegates
//! to the library facade [`unblock_cli::run`], returning its [`std::process::ExitCode`]. All routing,
//! logging, dispatch, and the 0–8 exit-code boundary live in the library so they stay hermetically
//! testable. See `docs/plans/crates/unblock-cli.md`.
#![forbid(unsafe_code)]

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    unblock_cli::run().await
}
