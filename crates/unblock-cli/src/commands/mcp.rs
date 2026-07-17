//! `unblock mcp` (FR-20, D27/AD-4, D38) — the PRIMARY runtime command.
//!
//! Opens a `WorkspaceContext`, installs the FR-17 shutdown handle, opens the `Session` wired to the
//! handle's flag, and runs the LIVE 2-arg `unblock_mcp::run_mcp_server(Arc<Session>, McpServerOptions)`
//! (transport internal `stdio()`). stdout carries ONLY MCP framing (logging is stderr-only, NFR-14).
//!
//! **Cancellation is TWO-outcome (spine §0.1 — D38 corrected the earlier "returns `Ok`" claim).** On
//! EOF / first signal the `CancellationToken` cancels and `run_mcp_server` returns EITHER:
//! - `Ok(())` — the rmcp `initialize` handshake had already completed; or
//! - `Err(McpServerError::Transport{Cancelled})` — the cancel landed DURING the handshake (rmcp 1.7
//!   wraps the WHOLE handshake in a `select!` against the token).
//!
//! BOTH are normal cooperative-shutdown outcomes, both MUST reach `session.shutdown()` (drain the
//! write permit, clean libsql close), and the `Err` MUST NOT be treated as an independent fault.
//!
//! **The signal wins, scoped to those two cooperative returns (D38, spine §5b).** If the FR-17 handle
//! recorded a signal, the command yields `Some(128+signo)` even when the run loop or the teardown
//! returned `Err` — that error is a CONSEQUENCE of the operator's cancellation, so it is emitted here
//! as a human `error[CODE]: message` diagnostic on stderr and never decides the exit code (blaming
//! `InternalError` on a process for obeying SIGTERM is what this command used to do). The precedence
//! binds ONLY those two returns: a failure raised BEFORE the run loop starts (`Session::open`, the
//! config open) is not a consequence of the cancellation and keeps its spine §2.3 0–8 code, so a
//! coinciding signal can never mask an unrelated DB fault. With NO signal recorded a genuine `Err`
//! keeps `InternalError`/exit 1, rendered by `exit::into_exit` per FR-11 — never swallowed.
//!
//! The exit is returned as a VALUE (`Ok(Some(128+signo))` through `run_with` → `ExitCode`), not a
//! `std::process::exit`: the decision therefore lives in the pure, unit-testable [`resolve_mcp_exit`]
//! rather than in a branch observable only end-to-end (which is precisely how D38's defect hid). The
//! peer no-hang half of D38 lives in `main.rs` (runtime ownership).

use std::sync::Arc;

use snafu::ResultExt;
use unblock_config::{CliOverrides, open_with_storage_with_cli};
use unblock_engine::Session;
use unblock_mcp::{McpServerOptions, Quotas, run_mcp_server};

use crate::cli::McpArgs;
use crate::dispatch::session_config;
use crate::exit::{CliError, McpSnafu};
use crate::shutdown;

/// What the `mcp` command should do once the run loop AND the teardown have both returned — the
/// output of the pure [`resolve_mcp_exit`] decision (D38, spine §5b).
#[derive(Debug)]
enum McpExit {
    /// A signal was recorded → exit `128+signo`. `diagnostics` carries the cooperative-shutdown
    /// errors (run loop first, then teardown) to be REPORTED on stderr but never to decide the code.
    Signal {
        /// The conventional `128+signo` code recorded by the FR-17 handle.
        code: u8,
        /// The post-signal errors — a consequence of the cancellation, diagnostics only.
        diagnostics: Vec<CliError>,
    },
    /// No signal was recorded → the outcome itself decides (the spine §2.3 0–8 cast, or exit 0).
    Outcome(Result<Option<u8>, CliError>),
}

/// Decide the `mcp` exit from the recorded signal + the two COOPERATIVE-SHUTDOWN returns (D38).
///
/// PURE (no I/O, no `process::exit`) so the precedence rule is unit- and mutation-testable rather
/// than only observable end-to-end — the D38 design gate's stated reason for this mechanism.
///
/// - `signal` — `handle.signal_exit_code()`: `Some(128+signo)` iff a signal was recorded.
/// - `run` — the `run_mcp_server` return (`Ok`, or a post-cancel `Err(Transport{Cancelled})`).
/// - `teardown` — the `session.shutdown()` return.
///
/// The signal takes precedence over BOTH returns; with no signal, `run`'s error wins over
/// `teardown`'s (the run loop failed first, so it is the root cause), and a clean pair is exit 0.
/// Callers must only pass returns from AFTER the run loop started — a pre-run-loop failure (e.g.
/// `Session::open`) is NOT a consequence of the cancellation and must keep its own 0–8 code.
fn resolve_mcp_exit(
    signal: Option<u8>,
    run: Result<(), CliError>,
    teardown: Result<(), CliError>,
) -> McpExit {
    match signal {
        Some(code) => McpExit::Signal {
            code,
            diagnostics: [run.err(), teardown.err()].into_iter().flatten().collect(),
        },
        None => McpExit::Outcome(run.and(teardown).map(|()| None)),
    }
}

/// Run `unblock mcp`.
///
/// # Errors
/// - [`CliError::Config`] if the workspace cannot be opened (discovery/DB/migrate);
/// - [`CliError::Engine`] if the session cannot be opened;
/// - [`CliError::Mcp`] if the MCP server fails to start or its run loop ends abnormally, or
///   [`CliError::Engine`] if `shutdown()` fails — **unless a signal was recorded**, in which case
///   the command returns `Ok(Some(128+signo))` and those errors are reported as stderr diagnostics
///   (D38 — see the module docs).
pub async fn run(_args: &McpArgs, overrides: &CliOverrides) -> Result<Option<u8>, CliError> {
    let ctx = open_with_storage_with_cli(overrides).await?;
    let cfg = session_config(&ctx);

    // Install the shutdown handle BEFORE opening the session so the flag is wired from the first tick.
    let handle = shutdown::install();

    // PRE-run-loop (D38 scope boundary): these `?`s are correct. A failure here is NOT a consequence
    // of a cancellation — even if a signal races it — so it keeps its spine §2.3 0–8 code and must
    // NOT be masked as `128+signo`.
    let session = Session::open(ctx, cfg)
        .await?
        .with_shutdown_flag(handle.flag.clone());
    let session = Arc::new(session);

    let opts = McpServerOptions {
        instructions: Some(instructions()),
        quotas: Quotas::default(),
        cancel: handle.token.clone(),
    };

    // Runs until EOF / cancellation. Deliberately NOT `?`: on the signal path this returns
    // `Err(Transport{Cancelled})` whenever the cancel landed during the rmcp handshake, and an
    // early-return here would bypass BOTH the teardown below and the recorded signal — the exact
    // D38 defect (exit 1 + a hang, instead of 128+signo).
    let run_result = run_mcp_server(Arc::clone(&session), opts)
        .await
        .context(McpSnafu);

    // Reached on BOTH cooperative returns (Ok and post-cancel Err) — an `Err(Cancelled)` never skips
    // the clean libsql close (spine §0.1/§4.2): drain the in-flight write permit, leave libsql idle.
    let teardown_result = session.shutdown().await.map_err(CliError::from);

    match resolve_mcp_exit(handle.signal_exit_code(), run_result, teardown_result) {
        McpExit::Signal { code, diagnostics } => {
            // `Ok(Some(_))` bypasses `exit::into_exit`, so nothing downstream would render these:
            // report them HERE or they would be silently swallowed (D38).
            for err in diagnostics {
                crate::exit::emit_diagnostic(err);
            }
            Ok(Some(code))
        }
        McpExit::Outcome(outcome) => outcome,
    }
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
    use super::{McpExit, instructions, resolve_mcp_exit};
    use crate::exit::CliError;
    use unblock_error::ErrorCode;

    #[test]
    fn instructions_mention_the_contract_version() {
        let text = instructions();
        assert!(text.contains(unblock_mcp::CONTRACT_VERSION));
        assert!(text.contains("MCP stdio"));
    }

    /// A REAL post-cancel run-loop error: `McpServerError::Transport` — exactly what rmcp returns
    /// when a cancel lands during the `initialize` handshake (spine §0.1). Uses the same `test-util`
    /// seam `exit.rs`'s D27/AF-4 cases use, so these are not straw-man errors.
    fn cancelled_run_error() -> CliError {
        CliError::Mcp {
            source: unblock_mcp::McpServerError::__transport_error("Cancelled"),
        }
    }

    /// A REAL teardown error shape (a `CliError::Engine`, what `session.shutdown()` surfaces).
    fn teardown_error() -> CliError {
        CliError::Io {
            source: std::io::Error::other("libsql close failed"),
        }
    }

    // -- D38 clause (1): the signal wins over BOTH cooperative-shutdown returns. -------------------

    /// The DEFECT case, pinned at the decision itself: a signal recorded + the run loop returning
    /// `Err(Cancelled)` (the pre-handshake window) must yield `128+signo`, NOT the error's exit 1.
    /// Reverting the precedence (consulting the error before the signal) turns this RED — the unit
    /// peer of `mcp_lifecycle.rs`'s e2e pre-handshake case.
    #[test]
    fn a_recorded_signal_beats_a_post_cancel_run_loop_error() {
        let exit = resolve_mcp_exit(Some(143), Err(cancelled_run_error()), Ok(()));
        match exit {
            McpExit::Signal { code, diagnostics } => {
                assert_eq!(
                    code, 143,
                    "the recorded signal decides the exit, not the Err"
                );
                assert_eq!(
                    diagnostics.len(),
                    1,
                    "the post-cancel Err is still REPORTED (never swallowed), just not the exit code"
                );
                assert_eq!(diagnostics[0].code(), ErrorCode::InternalError);
            }
            McpExit::Outcome(other) => {
                panic!("a recorded signal must not fall through to the 0-8 cast: {other:?}")
            }
        }
    }

    /// The teardown return is equally covered: a `session.shutdown()` failure observed AFTER a signal
    /// is a consequence of the cancellation, so it is a diagnostic — never the exit code.
    #[test]
    fn a_recorded_signal_beats_a_teardown_error() {
        match resolve_mcp_exit(Some(130), Ok(()), Err(teardown_error())) {
            McpExit::Signal { code, diagnostics } => {
                assert_eq!(code, 130);
                assert_eq!(diagnostics.len(), 1, "the teardown Err is reported");
            }
            McpExit::Outcome(other) => panic!("the signal must win over teardown: {other:?}"),
        }
    }

    /// BOTH cooperative returns failing after a signal: still `128+signo`, and BOTH errors are
    /// reported — the "never swallowed" clause holds for each independently.
    #[test]
    fn a_recorded_signal_reports_both_cooperative_errors_and_still_exits_128_plus_signo() {
        match resolve_mcp_exit(Some(143), Err(cancelled_run_error()), Err(teardown_error())) {
            McpExit::Signal { code, diagnostics } => {
                assert_eq!(code, 143);
                assert_eq!(diagnostics.len(), 2, "neither error may be dropped");
                assert_eq!(diagnostics[0].code(), ErrorCode::InternalError, "run first");
                assert_eq!(diagnostics[1].code(), ErrorCode::IoError, "teardown second");
            }
            McpExit::Outcome(other) => panic!("the signal must win: {other:?}"),
        }
    }

    /// The signal path is signo-generic (SIGHUP→129 / SIGINT→130 / SIGTERM→143), not SIGTERM-special.
    #[test]
    fn the_signal_exit_is_signo_generic() {
        for code in [129u8, 130, 143] {
            match resolve_mcp_exit(Some(code), Err(cancelled_run_error()), Ok(())) {
                McpExit::Signal { code: got, .. } => assert_eq!(got, code),
                McpExit::Outcome(other) => panic!("signal {code} must win: {other:?}"),
            }
        }
    }

    /// A clean signalled shutdown (both returns Ok) yields `128+signo` with NOTHING on stderr — the
    /// diagnostics are for real errors only, so a normal SIGTERM stays quiet.
    #[test]
    fn a_clean_signalled_shutdown_yields_the_signal_code_with_no_diagnostics() {
        match resolve_mcp_exit(Some(143), Ok(()), Ok(())) {
            McpExit::Signal { code, diagnostics } => {
                assert_eq!(code, 143);
                assert!(diagnostics.is_empty(), "no error → no diagnostic noise");
            }
            McpExit::Outcome(other) => panic!("the signal must decide: {other:?}"),
        }
    }

    // -- D38: with NO signal, a genuine Err keeps its 0-8 code (errors are never swallowed). -------

    /// The other half of the precedence: NO signal → the run-loop `Err` keeps `InternalError`/exit 1.
    /// Widening the guard to swallow unsignalled errors into a signal exit turns this RED.
    #[test]
    fn an_unsignalled_run_loop_error_keeps_its_0_8_code() {
        match resolve_mcp_exit(None, Err(cancelled_run_error()), Ok(())) {
            McpExit::Outcome(Err(err)) => {
                assert_eq!(err.code(), ErrorCode::InternalError);
                assert_eq!(err.code().exit_code(), 1, "the D27/AF-4 map still applies");
            }
            other => panic!("with no signal a genuine Err must propagate: {other:?}"),
        }
    }

    /// NO signal + a teardown failure → the teardown error propagates with its own code (exit 8 here),
    /// proving the teardown return is not silently discarded on the unsignalled path either.
    #[test]
    fn an_unsignalled_teardown_error_keeps_its_0_8_code() {
        match resolve_mcp_exit(None, Ok(()), Err(teardown_error())) {
            McpExit::Outcome(Err(err)) => assert_eq!(err.code(), ErrorCode::IoError),
            other => panic!("with no signal a teardown Err must propagate: {other:?}"),
        }
    }

    /// With NO signal and BOTH failing, the RUN-loop error wins: it failed first, so it is the root
    /// cause; reporting the teardown error would blame a downstream symptom.
    #[test]
    fn an_unsignalled_run_loop_error_wins_over_a_teardown_error() {
        match resolve_mcp_exit(None, Err(cancelled_run_error()), Err(teardown_error())) {
            McpExit::Outcome(Err(err)) => assert_eq!(
                err.code(),
                ErrorCode::InternalError,
                "the run-loop failure is the root cause, not the teardown symptom"
            ),
            other => panic!("expected the run-loop error: {other:?}"),
        }
    }

    /// The clean EOF path (no signal, both Ok) → `Ok(None)` → exit 0.
    #[test]
    fn a_clean_eof_exit_is_ok_none() {
        match resolve_mcp_exit(None, Ok(()), Ok(())) {
            McpExit::Outcome(Ok(None)) => {}
            other => panic!("a clean EOF shutdown is exit 0: {other:?}"),
        }
    }
}
