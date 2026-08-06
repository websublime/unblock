//! `unblock-cli` (L7) — the reduced lifecycle/ops CLI facade over the engine: `mcp`, `migrate`,
//! `doctor`, `version`, `init`, `agents`, `update` (D3). Thin routing + config flag-forwarding +
//! tracing + the 0–8 exit-code boundary. Owns cooperative-shutdown signal install (FR-17, OQ-4).
//!
//! The `unblock` binary entry point ([`src/main.rs`]) holds exactly one responsibility: it OWNS the
//! tokio runtime (building it, `block_on`-ing [`run`], then disposing of it NON-blockingly — the D38
//! no-hang invariant, spine §5b). [`run`] parses argv, initializes stderr-only logging, dispatches to
//! a command handler, and maps the outcome to a [`std::process::ExitCode`]. The library facade exists
//! so the routing + exit-code boundary are testable via [`run_with`] without spawning a process.
//!
//! **The CLI is a pure `CliOverrides` forwarder** (config owns ALL layering — D27/AD-3); the ONE
//! CLI-owned resolution seam is clap `env` binding. See `docs/plans/crates/unblock-cli.md` and the
//! spine §5b / §0.1.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod cli;
mod commands;
mod dispatch;
mod exit;
mod logging;
mod output;
mod shutdown;

use std::ffi::OsString;
use std::process::ExitCode;

use clap::Parser;

pub use cli::{Cli, Command};
pub use exit::CliError;

/// Parse `std::env::args_os()`, dispatch, and return the process exit code.
///
/// This is the entry point [`src/main.rs`] `block_on`s on the runtime it OWNS (D38 — NOT
/// `#[tokio::main]`, whose structurally unavoidable blocking runtime drop hung `unblock mcp`; see
/// the `main.rs` module docs). It never panics on a domain error — every failure is mapped to a
/// [`StructuredError`](unblock_error::StructuredError) and its 0–8 exit code at the [`exit`]
/// boundary (spine §2.4, conformance rule 6.5).
pub async fn run() -> ExitCode {
    run_with(std::env::args_os()).await
}

/// The hermetic, argv-injecting entry point (the `assert_cmd`-free path the exit-code tests drive).
///
/// Parses `args` with clap (a clap error — a usage error, `--help`, or `--version` — prints clap's own
/// message and returns clap's exit code: `0` for `--help`/`--version`, `2` for a usage error), then
/// initializes stderr-only logging, dispatches, and maps the outcome:
/// - `Ok(None)` → success (exit `0`);
/// - `Ok(Some(code))` → an explicit exit code (the `mcp` `128+signo` signal exit);
/// - `Err(e)` → [`exit::into_exit`] (the structured error render + 0–8 cast). Which STREAM that
///   render lands on is decided by the parsed command's [`Command::stdout_role`] (D48): STDOUT for a
///   command that owns stdout as its own report channel, STDERR for one that owns it as a
///   wire-protocol framing channel (`mcp`). The exit code is the same either way.
pub async fn run_with<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(err) => {
            // clap owns the message + stream (help/version → stdout, usage → stderr) and the exit
            // code (0 for DisplayHelp/DisplayVersion, 2 for a usage error). We do not call `.exit()`
            // (it terminates the process) — we must return an `ExitCode` from an async fn.
            let _ignored = err.print();
            return ExitCode::from(clap_exit_code(&err));
        }
    };

    // Initialize logging AFTER parse so `-v`/`-q` are known; stderr-only (NFR-14). Idempotent.
    logging::init_logging(cli.global.verbose, cli.global.quiet);

    // Resolve the error-render format up front from CLI+env only (config default is unknown before a
    // workspace opens; FR-13 precedence for the no-workspace error path — spine §5b, output.rs SF-1).
    let fmt = output::pick_cli_format(&cli.global);
    // D48: which CHANNEL that error render lands on, read HERE because `dispatch` consumes `cli`
    // and the fact is unrecoverable afterwards. `fmt` is deliberately untouched — degrading the
    // format for `mcp` would silently break FR-13 precedence (`-o json`, `UNBLOCK_OUTPUT_FORMAT`),
    // which is the rejected alternative D48 clause (2) names.
    let stdout_role = cli.command.stdout_role();

    match dispatch::dispatch(cli).await {
        Ok(None) => exit::ok_exit(),
        Ok(Some(code)) => ExitCode::from(code),
        Err(err) => exit::into_exit(err, fmt, stdout_role),
    }
}

/// Map a clap parse error to the process exit code: `0` for `--help`/`--version`, else `2` (usage).
fn clap_exit_code(err: &clap::Error) -> u8 {
    use clap::error::ErrorKind;
    match err.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => 0,
        _ => 2,
    }
}
