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
//! returned `Err` — that error is a CONSEQUENCE of the operator's cancellation, so it is REPORTED
//! here and never decides the exit code (blaming `InternalError` on a process for obeying SIGTERM is
//! what this command used to do). The precedence binds ONLY those two returns: a failure raised
//! BEFORE the run loop starts (`Session::open`, the config open) is not a consequence of the
//! cancellation and keeps its spine §2.3 0–8 code, so a coinciding signal can never mask an
//! unrelated DB fault. With NO signal recorded a genuine `Err` keeps `InternalError`/exit 1,
//! rendered by `exit::into_exit` — never swallowed. **Since D48 that render lands on STDERR**, not
//! on stdout: this command owns stdout as the JSON-RPC framing channel, so its structured error
//! (the FULL `code`/`message`/`hint`/`retryable` document — FR-11 is preserved in CONTENT and moved
//! in CHANNEL) goes where every other diagnostic already goes. The exit code is unmoved.
//!
//! **The unsignalled pre-`initialize` disconnect → exit 0 carve-out (D40, spine §5b).** One `Err` on
//! the unsignalled path is NOT a genuine fault: an `Err(McpServerError::Transport{ConnectionClosed})`
//! raised because the client closed the connection before completing the `initialize` handshake is a
//! routine lifecycle event (child-per-client, D31 — a peer that probes then leaves). So
//! [`resolve_mcp_exit`] intercepts it via [`unblock_mcp::McpServerError::is_pre_handshake_disconnect`]
//! and DELEGATES the exit code to the teardown: a clean `session.shutdown()` → `Ok(None)`/exit 0
//! (unifying with the post-handshake EOF), while a FAILING teardown still decides via its OWN 0–8 code
//! (a libsql-close fault → exit 8 is NEVER masked into exit 0). The disconnect is carried as a
//! `tracing::debug!` diagnostic (routed by [`diagnostic_route`], surfaced by `-vv`), never dropped.
//!
//! **How a reported error is LABELLED (D38 labelling clause).** "Never swallowed" is not "always
//! shout": a post-signal `Err(Transport{Cancelled})` is the cooperative shutdown SUCCEEDING, so
//! printing `error[INTERNAL_ERROR]` for it blames unblock for obeying — the very thing D38's
//! rationale objects to (D38 fixed the exit code and initially left the label). So each reported
//! error is ROUTED by the pure [`diagnostic_route`]: the cancellation class
//! ([`unblock_mcp::McpServerError::is_cancellation`]) goes to `tracing::debug!` (visible under
//! `-vv`, silent by default — it is routine), while a GENUINE error keeps its `error[CODE]: message`
//! stderr line (NFR-14). Nothing is dropped on either branch; only the volume differs.
//!
//! **Signal handling is installed FIRST — before the workspace opens (FR-17).** `install()` precedes
//! `open_with_storage_with_cli`, which does discovery + `LibsqlStorage::open_local` (taking the D31
//! `.write.lock`) + `migrate()`. A signal arriving in THAT window used to hit the default
//! disposition and HARD-KILL the process mid-`migrate()` — an FR-17 "unwinds cleanly" violation and
//! an integrity risk. Installed first, such a signal is instead RECORDED (the token cancels, the
//! flag sets): `migrate()` is not interrupted, it runs to completion, and the already-cancelled
//! token then makes `run_mcp_server` return `Err(Cancelled)` at once → the normal teardown → a clean
//! `128+signo`. The D38 scope boundary is unaffected: the open's own `?` still yields its 0–8 code.
//!
//! The exit is returned as a VALUE (`Ok(Some(128+signo))` through `run_with` → `ExitCode`), not a
//! `std::process::exit`: the decision therefore lives in the pure, unit-testable [`resolve_mcp_exit`]
//! rather than in a branch observable only end-to-end (which is precisely how D38's defect hid). The
//! peer no-hang half of D38 lives in `main.rs` (runtime ownership).

use std::sync::Arc;

use snafu::ResultExt;
use unblock_config::{CliOverrides, WorkspaceSource, open_with_storage_with_cli};
use unblock_engine::Session;
use unblock_mcp::{McpServerOptions, Quotas, run_mcp_server};

use crate::cli::McpArgs;
use crate::dispatch::session_config;
use crate::exit::{CliError, McpSnafu};
use crate::shutdown;

/// Where a reported error must GO (D38 labelling clause) — decided by the pure [`diagnostic_route`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiagnosticRoute {
    /// The CANCELLATION class: the cooperative shutdown working as designed. Routine, so it goes to
    /// `tracing::debug!` — present under `-vv`, silent at the default level. NOT dropped.
    Debug,
    /// A GENUINE fault: it keeps the human `error[CODE]: message` line on stderr (NFR-14).
    Stderr,
}

/// An error that MUST be reported but must NOT decide the exit code (D38 "never swallowed").
///
/// Two disjoint sources feed this: a post-signal cooperative-shutdown error (the signal decides the
/// code instead), and — on the UNSIGNALLED path — the teardown error that loses to the root-cause
/// run-loop error. Both used to vanish; neither may.
#[derive(Debug)]
struct Diagnostic {
    /// Where it goes (the labelling decision, pinned by [`diagnostic_route`]).
    route: DiagnosticRoute,
    /// The error itself — always carried, never discarded.
    error: CliError,
}

/// Route a reported error by CLASS: the rmcp cancellation outcome is routine, everything else is
/// genuine (D38 labelling clause; Miguel-ratified 2026-07-17).
///
/// PURE, so both branches are unit- and mutation-pinnable — a genuine post-signal error is not
/// deterministically reachable e2e (rmcp's outer `select!` returns `Cancelled` whenever the token
/// wins the handshake race), so this helper IS the coverage seam for the `Stderr` branch.
fn diagnostic_route(err: &CliError) -> DiagnosticRoute {
    match err {
        // TWO demotions, each narrow by construction: (i) a cancel that landed during the handshake
        // (`is_cancellation()` matches `Cancelled` alone — spine §0.1), and (ii) the pre-`initialize`
        // peer disconnect (`is_pre_handshake_disconnect()` matches `ConnectionClosed(_)` alone — D40).
        // Both are routine cooperative-shutdown outcomes, not faults, so they go to `tracing::debug!`;
        // neither predicate ever matches a real transport fault. (ii) also demotes a POST-signal
        // disconnect, resolving the D38 residual NIT where `Transport{ConnectionClosed}` still shouted.
        CliError::Mcp { source }
            if source.is_cancellation() || source.is_pre_handshake_disconnect() =>
        {
            DiagnosticRoute::Debug
        }
        _ => DiagnosticRoute::Stderr,
    }
}

/// Is `err` the pre-`initialize` peer disconnect (`Transport{ConnectionClosed}`)? The D40 seam
/// [`resolve_mcp_exit`] uses to intercept the UNSIGNALLED disconnect and delegate the exit code to the
/// teardown (a clean `session.shutdown()` → exit 0). PURE, so the interception is unit- and
/// mutation-pinnable; narrow by construction — `is_pre_handshake_disconnect()` matches
/// `ConnectionClosed(_)` alone, never `Cancelled` or a real transport fault.
fn is_mcp_disconnect(err: &CliError) -> bool {
    matches!(err, CliError::Mcp { source } if source.is_pre_handshake_disconnect())
}

/// Pair an error with its route (the classification step of [`resolve_mcp_exit`]).
fn classify(error: CliError) -> Diagnostic {
    Diagnostic {
        route: diagnostic_route(&error),
        error,
    }
}

/// What the `mcp` command should do once the run loop AND the teardown have both returned — the
/// output of the pure [`resolve_mcp_exit`] decision (D38, spine §5b).
#[derive(Debug)]
enum McpExit {
    /// A signal was recorded → exit `128+signo`. `diagnostics` carries the cooperative-shutdown
    /// errors (run loop first, then teardown) to be REPORTED but never to decide the code.
    Signal {
        /// The conventional `128+signo` code recorded by the FR-17 handle.
        code: u8,
        /// The post-signal errors — a consequence of the cancellation, diagnostics only.
        diagnostics: Vec<Diagnostic>,
    },
    /// No signal was recorded → the outcome itself decides (the spine §2.3 0–8 cast, or exit 0).
    Outcome {
        /// The code-deciding outcome: the root-cause error, or `Ok(None)` for a clean exit 0.
        outcome: Result<Option<u8>, CliError>,
        /// Errors that did NOT decide the code but must still be reported — i.e. a teardown error
        /// that lost to the run-loop root cause. Empty unless BOTH returns failed.
        diagnostics: Vec<Diagnostic>,
    },
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
/// `teardown`'s (the run loop failed first, so it is the root cause) — but the LOSING teardown error
/// is still reported as a diagnostic, never dropped (D38: errors are never swallowed; the signalled
/// path already reported both, and the unsignalled path must not be the asymmetric one). A clean
/// pair is exit 0.
///
/// Callers must only pass returns from AFTER the run loop started — a pre-run-loop failure (e.g.
/// `Session::open`) is NOT a consequence of the cancellation and must keep its own 0–8 code.
fn resolve_mcp_exit(
    signal: Option<u8>,
    run: Result<(), CliError>,
    teardown: Result<(), CliError>,
) -> McpExit {
    match signal {
        // The signal decides. BOTH cooperative errors become diagnostics, run loop first.
        Some(code) => McpExit::Signal {
            code,
            diagnostics: [run.err(), teardown.err()]
                .into_iter()
                .flatten()
                .map(classify)
                .collect(),
        },
        // No signal: the outcome decides, and nothing it displaces may be lost.
        None => match (run, teardown) {
            // D40 — the pre-`initialize` peer DISCONNECT is a routine lifecycle event, not a fault:
            // the run loop returned `Err(Transport{ConnectionClosed})` because the client closed the
            // connection before completing the `initialize` handshake. It must NOT cast to
            // `InternalError`/exit 1 — instead DELEGATE the exit code to the teardown: a clean
            // `session.shutdown()` → `Ok(None)`/exit 0 (unifying with the post-handshake EOF), while a
            // FAILING teardown still decides via its OWN 0–8 code (`teardown.map(|()| None)` keeps the
            // `Err`), so a libsql-close fault → exit 8 is NEVER masked into exit 0. The disconnect
            // itself is carried as a Debug diagnostic (`diagnostic_route` demotes it), never dropped.
            // This arm is FIRST so the disconnect is intercepted before the generic `(Err, _)` casts.
            (Err(run_err), teardown) if is_mcp_disconnect(&run_err) => McpExit::Outcome {
                outcome: teardown.map(|()| None),
                diagnostics: vec![classify(run_err)],
            },
            (Ok(()), Ok(())) => McpExit::Outcome {
                outcome: Ok(None),
                diagnostics: Vec::new(),
            },
            // BOTH failed: the run loop failed FIRST → it is the root cause and decides the code;
            // the teardown error is a downstream symptom, but it is still REPORTED.
            (Err(run_err), Err(teardown_err)) => McpExit::Outcome {
                outcome: Err(run_err),
                diagnostics: vec![classify(teardown_err)],
            },
            // Exactly one failed → it decides the code and nothing is displaced.
            (Err(err), Ok(())) | (Ok(()), Err(err)) => McpExit::Outcome {
                outcome: Err(err),
                diagnostics: Vec::new(),
            },
        },
    }
}

/// Report a diagnostic on the route its class earned — the ONE emission site (D38).
///
/// Neither branch drops the error: `Debug` records it through tracing (surfaced by `-vv`), `Stderr`
/// writes the `error[CODE]: message` line (NFR-14). stdout is never touched — on `mcp` it carries
/// MCP framing ONLY. **Since D48 that holds of the whole command ONCE THE SERVER STARTS, not just of
/// this function:** `exit::into_exit`'s machine arm — the last product writer that put a non-frame
/// document on fd 1 while the framing channel was live — now writes it to stderr too, so the
/// loophole this note used to describe around itself is closed.
///
/// Two qualifiers, both deliberate and both stated because an unqualified reading is falsified by
/// something shipped and green. `unblock mcp --help` never starts the server and legitimately prints
/// clap's usage prose to stdout (D48 clause 5; `tests/help_snapshots.rs`'s `mcp_help` asserts a
/// non-empty stdout there). And `output::emit_report` is still an unclassified stdout writer — it is
/// simply one this command never calls, which is a fact about the call graph and not a guarantee
/// (D48 clause 6(iii), tracked as `ub-c5o`).
fn report(diagnostic: Diagnostic) {
    match diagnostic.route {
        // Rendered via Debug (`?`) rather than Display so the line NAMES the rmcp outcome it demoted
        // (`Cancelled` / `ConnectionClosed`) and stays diagnosable — the two e2e peers assert on those
        // variant names. The message covers BOTH demoted outcomes and retains "cooperative shutdown".
        DiagnosticRoute::Debug => tracing::debug!(
            error = ?diagnostic.error,
            "the MCP run loop returned a cooperative-shutdown outcome — a post-signal cancellation \
             (D38), or an unsignalled pre-`initialize` client disconnect (D40) — a normal cooperative \
             shutdown, not a fault"
        ),
        DiagnosticRoute::Stderr => {
            crate::exit::emit_diagnostic(diagnostic.error, &mut std::io::stderr().lock());
        }
    }
}

/// Run `unblock mcp`.
///
/// # Errors
/// - [`CliError::Config`] if the workspace cannot be opened (discovery/DB/migrate);
/// - [`CliError::Engine`] if the session cannot be opened;
/// - [`CliError::Mcp`] if the MCP server fails to start or its run loop ends abnormally, or
///   [`CliError::Engine`] if `shutdown()` fails — **unless a signal was recorded**, in which case
///   the command returns `Ok(Some(128+signo))` and those errors are instead REPORTED (each on the
///   route its class earns: a cancellation via `tracing::debug!`, a genuine error on the
///   `error[CODE]` stderr line) — D38, see the module docs.
pub async fn run(_args: &McpArgs, overrides: &CliOverrides) -> Result<Option<u8>, CliError> {
    // FIRST — before ANY blocking/long work (FR-17). `open_with_storage_with_cli` below does
    // discovery + `open_local` (the D31 `.write.lock`) + `migrate()`; with no handler installed a
    // signal in that window hits the DEFAULT disposition and hard-kills the process mid-migrate.
    // Installed here, it is recorded instead and honoured at the first cooperative point.
    let handle = shutdown::install();
    // Ordering marker (pinned by `mcp_lifecycle.rs::shutdown_signal_handling_is_installed_before_
    // the_workspace_opens`): a signal is SAFE from this line on. Debug — routine, `-vv` only.
    tracing::debug!("mcp: shutdown signal handling installed");

    // PRE-run-loop (D38 scope boundary): these `?`s are correct. A failure here is NOT a consequence
    // of a cancellation — even if a signal races it — so it keeps its spine §2.3 0–8 code and must
    // NOT be masked as `128+signo`.
    let ctx = open_with_storage_with_cli(overrides).await?;
    // D39 startup visibility (NFR-14): ALWAYS report which workspace dir was bound and by which
    // discovery tier, on STDERR (an `info!` is silent at the default WARN level, so this is a
    // deliberate direct emit — not tracing). On `mcp` stdout is MCP framing ONLY (spine §5b). The
    // line never contains `error[` (the mcp e2e asserts `!stderr.contains("error[")`), so a routine
    // binding is not mistaken for a fault.
    emit_workspace_binding(
        &workspace_binding_line(&ctx.paths.unblock_dir, ctx.source),
        &mut std::io::stderr().lock(),
    );
    tracing::debug!("mcp: workspace opened");
    let cfg = session_config(&ctx);

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
    // D38 defect (exit 1 + a hang, instead of 128+signo). If a signal already landed during the
    // open above, the token is ALREADY cancelled here, so rmcp's `select!` returns `Err(Cancelled)`
    // immediately and this collapses onto the same cooperative path — no special case needed.
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
            for diagnostic in diagnostics {
                report(diagnostic);
            }
            Ok(Some(code))
        }
        McpExit::Outcome {
            outcome,
            diagnostics,
        } => {
            // `outcome`'s Err (if any) is rendered downstream by `exit::into_exit` — onto STDERR,
            // since this command owns stdout as the JSON-RPC framing channel (D48). These
            // are the errors it DISPLACED — nothing downstream will ever see them, so report them
            // here; the signalled arm above already did, and D38 tolerates no asymmetry.
            for diagnostic in diagnostics {
                report(diagnostic);
            }
            outcome
        }
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

/// The D39 startup-visibility line: the bound workspace dir + the winning discovery tier. PURE (no
/// I/O) so it is unit- and mutation-pinnable; [`emit_workspace_binding`] writes it verbatim. It must
/// never contain `error[` (the mcp e2e asserts `!stderr.contains("error[")`, so a routine binding is
/// not mistaken for a fault).
fn workspace_binding_line(unblock_dir: &std::path::Path, source: WorkspaceSource) -> String {
    format!(
        "unblock: workspace bound to {} (via {})",
        unblock_dir.display(),
        source.label()
    )
}

/// Write the D39 startup-visibility line to `out` (STDERR in production — NFR-14; on `mcp` stdout is
/// MCP framing ONLY, spine §5b). The sink is a PARAMETER so a unit test proves the bytes are actually
/// written (gutting this to a no-op turns the capture test RED). A failing stderr never changes the
/// exit code — the diagnostic is best-effort.
fn emit_workspace_binding(line: &str, out: &mut impl std::io::Write) {
    let _ignored = writeln!(out, "{line}");
}

#[cfg(test)]
mod tests {
    use super::{
        DiagnosticRoute, McpExit, diagnostic_route, emit_workspace_binding, instructions,
        resolve_mcp_exit, workspace_binding_line,
    };
    use crate::exit::CliError;
    use unblock_config::WorkspaceSource;
    use unblock_error::ErrorCode;

    #[test]
    fn instructions_mention_the_contract_version() {
        let text = instructions();
        assert!(text.contains(unblock_mcp::CONTRACT_VERSION));
        assert!(text.contains("MCP stdio"));
    }

    /// D39 (3): the startup line names BOTH the bound dir AND the winning tier, and never reads as a
    /// fault (the mcp e2e asserts `!stderr.contains("error[")`). A mutation that drops the dir or the
    /// tier from the format turns this RED.
    #[test]
    fn workspace_binding_line_names_the_dir_and_the_tier() {
        let line = workspace_binding_line(
            std::path::Path::new("/ws/.unblock"),
            WorkspaceSource::ProjectDir,
        );
        assert!(
            line.contains("workspace bound to /ws/.unblock"),
            "the bound dir must be named: {line}"
        );
        assert!(
            line.contains("via CLAUDE_PROJECT_DIR"),
            "the winning tier must be named: {line}"
        );
        assert!(
            !line.contains("error["),
            "a routine binding must not read as a fault: {line}"
        );
    }

    /// Each tier renders its human label — a mutation that collapses the labels turns this RED.
    #[test]
    fn workspace_binding_line_labels_every_tier() {
        let dir = std::path::Path::new("/ws/.unblock");
        assert!(
            workspace_binding_line(dir, WorkspaceSource::ExplicitDir).contains("--dir/UNBLOCK_DIR")
        );
        assert!(workspace_binding_line(dir, WorkspaceSource::ExplicitDb).contains("--db"));
        assert!(workspace_binding_line(dir, WorkspaceSource::WalkUp).contains("walk-up from cwd"));
    }

    /// The emitter actually WRITES the line to its sink (D39 visibility is only real if the bytes hit
    /// stderr) — gutting `emit_workspace_binding` to a no-op turns this RED.
    #[test]
    fn emit_workspace_binding_writes_the_line() {
        let mut out = Vec::new();
        emit_workspace_binding(
            &workspace_binding_line(
                std::path::Path::new("/ws/.unblock"),
                WorkspaceSource::WalkUp,
            ),
            &mut out,
        );
        let text = String::from_utf8(out).expect("utf8 line");
        assert_eq!(
            text, "unblock: workspace bound to /ws/.unblock (via walk-up from cwd)\n",
            "one newline-terminated line, verbatim"
        );
    }

    /// A REAL post-cancel run-loop error: the genuine `ServerInitializeError::Cancelled` rmcp
    /// returns when a cancel lands during the `initialize` handshake (spine §0.1), built through the
    /// same `test-util` seam `exit.rs`'s D27/AF-4 cases use — not a straw-man look-alike. This is
    /// the CANCELLATION class, so it routes to `tracing::debug!` (D38 labelling clause).
    fn cancelled_run_error() -> CliError {
        CliError::Mcp {
            source: unblock_mcp::McpServerError::__cancelled_error(),
        }
    }

    /// A REAL but GENUINE run-loop transport failure (a connection reset — NOT a cancellation).
    /// Post-signal it must keep its `error[CODE]` stderr line: the demotion is narrow.
    fn genuine_run_error() -> CliError {
        CliError::Mcp {
            source: unblock_mcp::McpServerError::__transport_error("connection reset"),
        }
    }

    /// A REAL pre-`initialize` DISCONNECT run-loop error: the genuine
    /// `ServerInitializeError::ConnectionClosed` rmcp returns when the peer closes the connection
    /// before completing the handshake (`receive()==None`), built through the same `test-util` seam.
    /// This is the D40 disconnect class → routes to `tracing::debug!` and, on the unsignalled path,
    /// delegates the exit code to the teardown.
    fn disconnect_run_error() -> CliError {
        CliError::Mcp {
            source: unblock_mcp::McpServerError::__connection_closed_error(),
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
                assert_eq!(diagnostics[0].error.code(), ErrorCode::InternalError);
                assert_eq!(
                    diagnostics[0].route,
                    DiagnosticRoute::Debug,
                    "a cancellation is the shutdown WORKING — routine, so it is not shouted (D38)"
                );
            }
            McpExit::Outcome { outcome, .. } => {
                panic!("a recorded signal must not fall through to the 0-8 cast: {outcome:?}")
            }
        }
    }

    /// The demotion is CLASS-based, not signal-based: a GENUINE post-signal failure still gets the
    /// loud `error[CODE]` line. Widening the demotion to "anything after a signal" turns this RED.
    #[test]
    fn a_genuine_post_signal_error_is_still_reported_loudly() {
        match resolve_mcp_exit(Some(143), Err(genuine_run_error()), Ok(())) {
            McpExit::Signal { code, diagnostics } => {
                assert_eq!(code, 143, "the signal still decides the code");
                assert_eq!(diagnostics.len(), 1, "and the genuine error is reported");
                assert_eq!(
                    diagnostics[0].route,
                    DiagnosticRoute::Stderr,
                    "a REAL fault that merely coincides with a signal must stay visible"
                );
            }
            McpExit::Outcome { outcome, .. } => panic!("the signal must win: {outcome:?}"),
        }
    }

    /// The routing decision itself, pinned on both branches at the pure helper (the seam that makes
    /// the `Stderr` branch provable — it is not deterministically reachable end-to-end).
    #[test]
    fn diagnostic_route_demotes_only_the_cancellation_class() {
        assert_eq!(
            diagnostic_route(&cancelled_run_error()),
            DiagnosticRoute::Debug,
            "the cancel-during-handshake outcome is routine"
        );
        assert_eq!(
            diagnostic_route(&genuine_run_error()),
            DiagnosticRoute::Stderr,
            "a genuine transport fault is NOT demoted"
        );
        assert_eq!(
            diagnostic_route(&teardown_error()),
            DiagnosticRoute::Stderr,
            "a teardown/IO fault is never a cancellation"
        );
    }

    /// The teardown return is equally covered: a `session.shutdown()` failure observed AFTER a signal
    /// is a consequence of the cancellation, so it is a diagnostic — never the exit code.
    #[test]
    fn a_recorded_signal_beats_a_teardown_error() {
        match resolve_mcp_exit(Some(130), Ok(()), Err(teardown_error())) {
            McpExit::Signal { code, diagnostics } => {
                assert_eq!(code, 130);
                assert_eq!(diagnostics.len(), 1, "the teardown Err is reported");
                assert_eq!(
                    diagnostics[0].route,
                    DiagnosticRoute::Stderr,
                    "a failed libsql close is a GENUINE fault even after a signal — stay loud"
                );
            }
            McpExit::Outcome { outcome, .. } => {
                panic!("the signal must win over teardown: {outcome:?}")
            }
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
                assert_eq!(
                    diagnostics[0].error.code(),
                    ErrorCode::InternalError,
                    "run first"
                );
                assert_eq!(
                    diagnostics[1].error.code(),
                    ErrorCode::IoError,
                    "teardown second"
                );
                // Mixed classes travel INDEPENDENTLY: the routine cancellation is demoted while the
                // genuine teardown fault stays loud, in the SAME shutdown.
                assert_eq!(diagnostics[0].route, DiagnosticRoute::Debug);
                assert_eq!(diagnostics[1].route, DiagnosticRoute::Stderr);
            }
            McpExit::Outcome { outcome, .. } => panic!("the signal must win: {outcome:?}"),
        }
    }

    /// The signal path is signo-generic (SIGHUP→129 / SIGINT→130 / SIGTERM→143), not SIGTERM-special.
    #[test]
    fn the_signal_exit_is_signo_generic() {
        for code in [129u8, 130, 143] {
            match resolve_mcp_exit(Some(code), Err(cancelled_run_error()), Ok(())) {
                McpExit::Signal { code: got, .. } => assert_eq!(got, code),
                McpExit::Outcome { outcome, .. } => {
                    panic!("signal {code} must win: {outcome:?}")
                }
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
            McpExit::Outcome { outcome, .. } => panic!("the signal must decide: {outcome:?}"),
        }
    }

    // -- D38: with NO signal, a genuine Err keeps its 0-8 code (errors are never swallowed). -------

    /// The other half of the precedence: NO signal → the run-loop `Err` keeps `InternalError`/exit 1.
    /// Widening the guard to swallow unsignalled errors into a signal exit turns this RED.
    #[test]
    fn an_unsignalled_run_loop_error_keeps_its_0_8_code() {
        match resolve_mcp_exit(None, Err(cancelled_run_error()), Ok(())) {
            McpExit::Outcome {
                outcome: Err(err),
                diagnostics,
            } => {
                assert_eq!(err.code(), ErrorCode::InternalError);
                assert_eq!(err.code().exit_code(), 1, "the D27/AF-4 map still applies");
                assert!(
                    diagnostics.is_empty(),
                    "nothing was displaced, so nothing is reported twice"
                );
            }
            other => panic!("with no signal a genuine Err must propagate: {other:?}"),
        }
    }

    /// NO signal + a teardown failure → the teardown error propagates with its own code (exit 8 here),
    /// proving the teardown return is not silently discarded on the unsignalled path either.
    #[test]
    fn an_unsignalled_teardown_error_keeps_its_0_8_code() {
        match resolve_mcp_exit(None, Ok(()), Err(teardown_error())) {
            McpExit::Outcome {
                outcome: Err(err),
                diagnostics,
            } => {
                assert_eq!(err.code(), ErrorCode::IoError);
                assert!(
                    diagnostics.is_empty(),
                    "it decided the code; it is not also a diagnostic"
                );
            }
            other => panic!("with no signal a teardown Err must propagate: {other:?}"),
        }
    }

    /// With NO signal and BOTH failing, the RUN-loop error wins the EXIT CODE: it failed first, so
    /// it is the root cause; blaming the teardown would blame a downstream symptom.
    ///
    /// **And the losing teardown error is still REPORTED (D38 — never swallowed).** This is the
    /// asymmetry the T3.2.1 Verify gate caught: `run.and(teardown)` DROPPED it on the floor here
    /// while the signalled path deliberately reported both, so a libsql-close failure vanished on
    /// the unsignalled path only. Reverting to `run.and(teardown)` turns this RED.
    #[test]
    fn an_unsignalled_run_loop_error_wins_but_the_teardown_error_is_still_reported() {
        match resolve_mcp_exit(None, Err(cancelled_run_error()), Err(teardown_error())) {
            McpExit::Outcome {
                outcome: Err(err),
                diagnostics,
            } => {
                assert_eq!(
                    err.code(),
                    ErrorCode::InternalError,
                    "the run-loop failure is the root cause, not the teardown symptom"
                );
                assert_eq!(
                    diagnostics.len(),
                    1,
                    "the DISPLACED teardown error must still be reported — nothing downstream will \
                     ever render it (D38: never swallowed)"
                );
                assert_eq!(
                    diagnostics[0].error.code(),
                    ErrorCode::IoError,
                    "and it is the teardown error, carried intact"
                );
                assert_eq!(diagnostics[0].route, DiagnosticRoute::Stderr);
            }
            other => panic!("expected the run-loop error: {other:?}"),
        }
    }

    /// The clean EOF path (no signal, both Ok) → `Ok(None)` → exit 0, silent.
    #[test]
    fn a_clean_eof_exit_is_ok_none() {
        match resolve_mcp_exit(None, Ok(()), Ok(())) {
            McpExit::Outcome {
                outcome: Ok(None),
                diagnostics,
            } => assert!(diagnostics.is_empty(), "a clean exit says nothing"),
            other => panic!("a clean EOF shutdown is exit 0: {other:?}"),
        }
    }

    // -- D40 (T3.2.1 follow-up (b)): unsignalled pre-`initialize` disconnect → the teardown decides. --

    /// D40 — with NO signal, an unsignalled pre-`initialize` DISCONNECT does NOT cast to exit 1: it is
    /// intercepted and the exit code is DELEGATED to the teardown. With a clean `session.shutdown()`
    /// the command exits 0 (`Ok(None)`), and the disconnect is still REPORTED as a Debug diagnostic
    /// (demoted, never dropped) — unifying with the post-handshake EOF exit 0.
    #[test]
    fn an_unsignalled_pre_handshake_disconnect_with_a_clean_teardown_exits_0() {
        match resolve_mcp_exit(None, Err(disconnect_run_error()), Ok(())) {
            McpExit::Outcome {
                outcome: Ok(None),
                diagnostics,
            } => {
                assert_eq!(
                    diagnostics.len(),
                    1,
                    "the disconnect is REPORTED (never dropped), just not as the exit code"
                );
                assert_eq!(
                    diagnostics[0].route,
                    DiagnosticRoute::Debug,
                    "a routine pre-`initialize` disconnect is demoted to -vv debug, not shouted"
                );
                assert_eq!(diagnostics[0].error.code(), ErrorCode::InternalError);
            }
            other => panic!("a clean-teardown disconnect must be exit 0 (D40): {other:?}"),
        }
    }

    /// D40 — the load-bearing correctness case: the interception DELEGATES to the teardown, it does NOT
    /// blanket `Ok(None)`. A pre-`initialize` disconnect whose `session.shutdown()` ALSO fails must let
    /// the TEARDOWN decide the exit code (exit 8 / `IoError` here), NEVER masking a libsql-close fault
    /// into exit 0. **A blanket-`Ok(None)` mutation in the interception arm turns THIS red.** The
    /// disconnect is still reported as a Debug diagnostic (never swallowed).
    #[test]
    fn an_unsignalled_disconnect_with_a_failing_teardown_lets_the_teardown_decide() {
        match resolve_mcp_exit(None, Err(disconnect_run_error()), Err(teardown_error())) {
            McpExit::Outcome {
                outcome: Err(err),
                diagnostics,
            } => {
                assert_eq!(
                    err.code(),
                    ErrorCode::IoError,
                    "a failing teardown decides via its OWN 0–8 code — the disconnect never masks it"
                );
                assert_eq!(
                    err.code().exit_code(),
                    8,
                    "a libsql-close fault must still exit 8, NEVER be swallowed into exit 0 (D40)"
                );
                assert_eq!(
                    diagnostics.len(),
                    1,
                    "and the disconnect is still REPORTED as a diagnostic"
                );
                assert_eq!(
                    diagnostics[0].route,
                    DiagnosticRoute::Debug,
                    "the disconnect itself is the routine outcome — demoted"
                );
                assert_eq!(diagnostics[0].error.code(), ErrorCode::InternalError);
            }
            other => panic!("a failing teardown must decide the code, not exit 0 (D40): {other:?}"),
        }
    }

    /// D40 routing at the pure helper: a pre-`initialize` disconnect is demoted to `Debug` exactly like
    /// a cancellation, while genuine faults stay `Stderr`. Reverting the `diagnostic_route` extension
    /// (dropping the `is_pre_handshake_disconnect()` guard) turns this RED.
    #[test]
    fn diagnostic_route_demotes_the_pre_handshake_disconnect() {
        assert_eq!(
            diagnostic_route(&disconnect_run_error()),
            DiagnosticRoute::Debug,
            "a pre-`initialize` disconnect is a routine cooperative-shutdown outcome (D40)"
        );
        assert_eq!(
            diagnostic_route(&genuine_run_error()),
            DiagnosticRoute::Stderr,
            "a genuine transport fault is NOT demoted"
        );
    }
}
