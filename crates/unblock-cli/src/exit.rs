//! The 0–8 exit-code boundary (spine §2.3 / conformance rule 6.5) — the CLI's `CliError` and the
//! `CliError → StructuredError → std::process::ExitCode` cast (the CLI OWNS the cast; there is no
//! `From<ExitCode> for std::process::ExitCode` in `unblock-error`).
//!
//! **Error mapping (D27/AF-4):** `EngineError`/`ConfigError`/`RenderError` are `CodedError`, so they
//! bridge via `(&err).into()` (the blanket `From<&E>`). `McpServerError` is the deliberate exception:
//! it does NOT impl `CodedError`, so `exit.rs` maps `Transport`/`RunLoop` EXPLICITLY to
//! `ErrorCode::InternalError` (exit 1) — an MCP-server run-loop/transport failure is an INTERNAL condition,
//! not a user `IoError` (exit 8). CLI-local variants: `AlreadyInitialized` (exit 2, the init clobber
//! guard — `ConfigError` has none), scaffold/agents `Io` (exit 8), `Update` (exit 1).
//!
//! **NFR-14 + FR-11 stream split:** in `json`/`robot` the structured error renders to the command's
//! REPORT channel (always valid JSON even on error, FR-11); in `plain`/`csv`/`markdown` a human
//! `error[CODE]: message` line goes to STDERR (diagnostics, NFR-14).
//!
//! **D48 — which stream the report channel IS.** For every command but one it is STDOUT, exactly
//! as NFR-14's generic reading says. (No count is stated: `Command::Update` is behind the default-on
//! `self-update` feature, so the subcommand enum has a different size under
//! `--no-default-features` and any number written here would be false in one of the two builds.)
//! For a command that owns stdout as a wire-protocol FRAMING
//! channel ([`StdoutRole::Protocol`] — `unblock mcp` on MCP stdio, the only member today) every byte
//! on fd 1 must be a JSON-RPC frame, so the SAME structured document goes to STDERR instead, whole
//! and undegraded. The CHANNEL moves; the payload and the 0–8 exit codes do not.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use snafu::Snafu;
use unblock_error::{ErrorCode, StructuredError};
use unblock_render::{OutputFormat, RenderOptions, renderer_for};

/// D48: what a command's STDOUT *is* — its own report channel, or a wire-protocol framing channel.
///
/// The rule is stated over this PROPERTY rather than over a command NAME so a future
/// protocol-owning command inherits it instead of re-deriving it (PRD §4 D48 clause 1). It is a
/// two-valued ENUM and never a `bool`: that makes both call sites self-describing and makes a
/// flipped classification a visible one-token edit rather than an invisible `!`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StdoutRole {
    /// stdout is this command's OWN report channel: the `json`/`robot` structured error renders
    /// there (NFR-14's generic rule — `version`, `migrate`, `doctor`, `init`, `agents`, plus
    /// `update` whenever the default-on `self-update` feature is enabled; under
    /// `--no-default-features` that variant does not exist and its arm is `cfg`-gated out).
    Reports,
    /// stdout is a wire-protocol FRAMING channel: every byte on it must be a JSON-RPC frame, so the
    /// structured error renders to STDERR instead (D48). `unblock mcp` is the only member today.
    Protocol,
}

/// The `unblock-cli` error type — the single L7 surface mapped to a 0–8 exit code (spine §2.3).
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum CliError {
    /// An engine error (transparent — forwards its `CodedError` code).
    #[snafu(transparent)]
    Engine {
        /// The underlying engine failure.
        source: unblock_engine::EngineError,
    },

    /// A config-resolution / workspace-open error (transparent — forwards its `CodedError` code).
    #[snafu(transparent)]
    Config {
        /// The underlying config failure.
        source: unblock_config::ConfigError,
    },

    /// A render failure at the CLI boundary (rare — a bad `-o` is caught by clap). `RenderError`
    /// impls `CodedError` (D27/AF-4) but we keep the arm explicit for clarity.
    #[snafu(display("failed to render output: {source}"))]
    Render {
        /// The underlying render failure.
        source: unblock_render::RenderError,
    },

    /// The MCP server failed to start or its run loop ended abnormally (FR-20). `McpServerError` has
    /// NO `CodedError`; it is mapped EXPLICITLY to `InternalError` (exit 1) — an MCP-server failure is
    /// internal, not a user I/O fault (D27/AF-4).
    #[snafu(display("mcp server error: {source}"))]
    Mcp {
        /// The underlying MCP server lifecycle failure (`Transport`/`RunLoop`).
        source: unblock_mcp::McpServerError,
    },

    /// `unblock init` clobber guard: a `config.toml` or `unblock.db` is already present without
    /// `--force` (AF-3). Maps to `ErrorCode::AlreadyInitialized` (exit 2); `ConfigError` has no such
    /// variant, so this is CLI-local.
    #[snafu(display("workspace already initialized at {}", path.display()))]
    AlreadyInitialized {
        /// The `.unblock/` directory that already contains a scaffold.
        path: PathBuf,
    },

    /// A CLI-local file-system operation failed (scaffold write / `AGENTS.md` write). Maps to
    /// `ErrorCode::IoError` (exit 8).
    #[snafu(display("file operation failed: {source}"))]
    Io {
        /// The underlying I/O failure.
        source: std::io::Error,
    },

    /// A self-update failure (`axoupdater`/dist-installer). Maps to `ErrorCode::InternalError` (exit 1).
    #[cfg(feature = "self-update")]
    #[snafu(display("self-update failed: {message}"))]
    Update {
        /// A human description of the update failure.
        message: String,
    },
}

impl CliError {
    /// The stable [`ErrorCode`] this error maps to (spine §2.3 — the single-valued code map).
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        use unblock_error::CodedError as _;
        match self {
            // Transparent-CodedError sources forward their own code.
            Self::Engine { source } => source.code(),
            Self::Config { source } => source.code(),
            Self::Render { source } => source.code(),
            // McpServerError has NO CodedError — an MCP-server failure is internal (exit 1), not I/O.
            Self::Mcp { .. } => ErrorCode::InternalError,
            Self::AlreadyInitialized { .. } => ErrorCode::AlreadyInitialized,
            Self::Io { .. } => ErrorCode::IoError,
            #[cfg(feature = "self-update")]
            Self::Update { .. } => ErrorCode::InternalError,
        }
    }
}

/// Build the `StructuredError` payload for a `CliError` (spine §2.4). Transparent-`CodedError` arms
/// use the blanket `(&source).into()` (so hint/context/retryable ride along); the explicit arms use
/// `from_code` (which routes the message through `sanitize_message`). Consumes `err` (this is the
/// terminal boundary sink), moving the source out of each transparent arm.
fn to_structured(err: CliError) -> StructuredError {
    // Compute the code + display message BEFORE moving out of `err` (both borrow `&err`).
    let code = err.code();
    let message = err.to_string();
    match err {
        CliError::Engine { source } => (&source).into(),
        CliError::Config { source } => (&source).into(),
        CliError::Render { source } => (&source).into(),
        // Explicit / CLI-local arms: `from_code` sets `retryable = code.is_retryable()` + sanitizes.
        _ => StructuredError::from_code(code, message),
    }
}

/// Convert a `CliError` into the process `ExitCode`, emitting the structured error per NFR-14/FR-11.
///
/// - `json`/`robot`: the `StructuredError` renders to the command's REPORT channel — STDOUT for a
///   [`StdoutRole::Reports`] command, STDERR for a [`StdoutRole::Protocol`] one (D48). Either way it
///   is the SAME document: always valid JSON even on error, `hint` retained;
/// - `plain`/`csv`/`markdown`: a human `error[CODE]: message` line goes to STDERR (diagnostics) —
///   role-independent, since that stream is already correct under both.
///
/// The 0–8 cast is CLI-owned and lives HERE: this wrapper is the single `ExitCode::from` site over
/// [`into_exit_to`]'s raw byte. `role` is derived from the PARSED command
/// ([`crate::cli::Command::stdout_role`]) at `lib.rs`, where `cli` is still in scope.
///
/// **Honest residue (the reason [`into_exit_to`] exists at all is testability, so its limit is
/// stated rather than left to be discovered):** binding the two real streams below is the ONE line
/// no unit cell can pin — swapping these two arguments compiles and survives every cell that drives
/// the core against buffers. That is what the spawning end-to-end suite
/// (`tests/mcp_stdout_channel.rs`) and the two inverted `tests/mcp_lifecycle.rs` cells cover, and
/// why D48 clause (7) demands BOTH layers rather than treating either as sufficient.
#[must_use]
pub(crate) fn into_exit(err: CliError, fmt: OutputFormat, role: StdoutRole) -> ExitCode {
    ExitCode::from(into_exit_to(
        err,
        fmt,
        role,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    ))
}

/// The sink-injected core of [`into_exit`], returning the RAW 0–8 code.
///
/// **Why both sinks are PARAMETERS.** [`into_exit`] writes through `std::io::stdout().lock()`, which
/// bypasses libtest's capture, and `#![forbid(unsafe_code)]` with no `libc` dependency rules out
/// redirecting the descriptor — so the stream CHOICE is observable in-process only through injection
/// (the same argument [`emit_diagnostic`] already makes for itself below). The STDOUT sink is
/// injected as well as the stderr one for the `Reports` MIRROR: that a non-protocol command's bytes
/// still land on stdout, unmoved. `U2` below is the cell written FOR that mirror and is the only one
/// here whose entire purpose it is.
///
/// **It is not the only cell that reads the stdout sink, and this used to say it was.** That
/// exclusivity is the sentence a later reader would act on when deleting a "redundant" cell, so what
/// actually happens is recorded instead, MEASURED by applying the mutation the mirror exists to
/// catch — rewriting the `Reports` arm below to write to `stderr` — and reading the failures off:
/// `U2` fails as the mirror, `U8` fails because comparing the two sink buffers against each other
/// starts by asserting the stdout one is NON-EMPTY, and `U9` fails because it reads the terminator
/// off whichever sink `role` selected. Each has its own reason to need the sink, which is why none
/// of the three is redundant with the others.
///
/// **Why it returns `u8` and not `ExitCode`.** `std::process::ExitCode` implements neither
/// `PartialEq` nor any numeric accessor, which would leave D48 clause (4) — the exit code does NOT
/// move when the channel does — unassertable in-process. The single cast stays in the wrapper.
#[must_use]
pub(crate) fn into_exit_to(
    err: CliError,
    fmt: OutputFormat,
    role: StdoutRole,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> u8 {
    let structured = to_structured(err);
    let exit = structured.exit_code();

    match fmt {
        // Machine formats: the structured payload to the command's REPORT channel (FR-11
        // always-valid JSON on error). D48 chooses the stream from `role` and changes NOTHING else —
        // the same bytes, the same `hint`, the same `exit` returned below.
        OutputFormat::Json | OutputFormat::Robot => {
            let opts = RenderOptions::default();
            if let Ok(out) = renderer_for(fmt, opts.clone()).structured_error(&structured, &opts) {
                match role {
                    StdoutRole::Reports => write_payload(&out.stdout, stdout),
                    // stdout is the JSON-RPC framing channel here: a StructuredError document is not
                    // a frame, so it goes to stderr — whole, where an MCP host capturing the child's
                    // stderr can still read it.
                    StdoutRole::Protocol => write_payload(&out.stdout, stderr),
                }
            } else {
                // Rendering the error itself failed — still surface something machine-safe on
                // stderr. Deliberately does NOT branch on `role`: this destination is already
                // correct under both, since stderr is never a framing channel.
                let _ignored = write_human(&structured, stderr);
            }
        }
        // Human formats: a one-line diagnostic to STDERR (NFR-14) — role-independent, same reason.
        _ => {
            let _ignored = write_human(&structured, stderr);
        }
    }

    exit
}

/// Write a rendered machine payload plus its terminating newline onto `out` — the ONE place that
/// shape exists, so the two [`StdoutRole`] arms cannot drift in anything but their destination.
///
/// **The TERMINATOR is part of the contract, not decoration, and it is pinned by its own cell (U9
/// below) rather than left to the byte-equality one.** The renderer returns `serde_json::to_string`
/// output, which carries no trailing newline; both consumers read this stream LINE-WISE — an MCP
/// host tailing the child's stderr (D48), and every shell pipeline reading a `Reports` command's
/// stdout — so dropping it would withhold the payload until EOF on BOTH channels. `U8`, which
/// compares the two channels against each other, structurally cannot see that: this single site
/// mutates both of its operands identically.
///
/// A failing report stream never changes the exit code we were asked to deliver, so both writes are
/// deliberately ignored (the same rule [`emit_diagnostic`] states).
fn write_payload(payload: &str, out: &mut impl Write) {
    let _ignored = out.write_all(payload.as_bytes());
    let _ignored = out.write_all(b"\n");
}

/// Report a `CliError` as a human `error[CODE]: message` line on `out` **without deciding the exit
/// code** (D38, spine §5b) — the `mcp` command's POST-SIGNAL and DISPLACED-teardown diagnostic sink.
///
/// **Since D48 this is one of TWO `mcp` stderr writers, and the pair differs in SHAPE, not in
/// channel.** [`into_exit`] renders the unsignalled `Err` path's FULL structured document to stderr
/// on this command; this function renders the degraded one-line form. The asymmetry is FROZEN by
/// decision rather than repaired here (PRD §4 D48 clause 2): the lines below are not the report of
/// the process's OUTCOME — the exit code was already decided by the recorded signal (D38) or by the
/// root-cause error that displaced them — and they already went to stderr, so no channel moves.
/// Widening them to the whole document would be a payload change on a D38-owned path.
///
/// When the FR-17 handle recorded a signal, `commands/mcp.rs` returns `Ok(Some(128+signo))`, which
/// takes `run_with`'s Ok arm and so never reaches [`into_exit`]; likewise the teardown error that
/// the UNSIGNALLED path discards in favour of the root-cause run-loop error is returned by nobody.
/// Such an error must still be surfaced (never swallowed) but must not blame the process for obeying
/// the signal. Routing through the same `to_structured` + [`write_human`] pair as the error path
/// keeps the two renderings from drifting (one sanitize site, one line shape).
///
/// **Why the sink is a PARAMETER and not a hard-coded `eprintln!`.** This function carries a
/// NORMATIVE D38 clause ("genuine errors are surfaced, never swallowed") whose only proof is that
/// the bytes are actually written. With the sink injected, the unit test drives THIS function
/// against a buffer, so gutting it to a no-op turns that test RED — the mutation that previously
/// SURVIVED the whole suite. A pure-formatter split would not achieve that: gutting the emitter
/// would still leave the formatter's test green. Callers pass STDERR: on `mcp`, stdout is MCP
/// framing ONLY (NFR-14) ONCE THE SERVER STARTS — since D48 the unsignalled `Err` path writes its
/// structured document to stderr too (via [`into_exit`], in the full shape this function does not
/// use), so the framing channel is no longer the exception it once was.
///
/// The qualifier is D48 clause (5)'s and is load-bearing rather than cautious: `unblock mcp --help`
/// never starts the server, so clap prints its usage prose to stdout and exits 0, and
/// `tests/help_snapshots.rs`'s `mcp_help` asserts exactly that. "On every path" — which this note
/// used to say — is refuted by a shipped green test.
pub(crate) fn emit_diagnostic(err: CliError, out: &mut impl Write) {
    // A diagnostic must never itself become a failure: a closed/failing stderr is not a reason to
    // change the exit code we were asked to deliver.
    let _ignored = write_human(&to_structured(err), out);
}

/// Render the single human diagnostic line shape (`error[CODE]: message`) onto `out` — the ONE
/// place that shape exists, shared by [`into_exit_to`]'s human arm, its render-failure fallback arm,
/// and [`emit_diagnostic`].
fn write_human(structured: &StructuredError, out: &mut impl Write) -> std::io::Result<()> {
    writeln!(
        out,
        "error[{}]: {}",
        structured.code.as_str(),
        structured.message
    )
}

/// The success exit code (`0`).
#[must_use]
pub(crate) fn ok_exit() -> ExitCode {
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::{CliError, to_structured};
    use unblock_error::ErrorCode;

    #[test]
    fn already_initialized_maps_to_exit_2() {
        let err = CliError::AlreadyInitialized {
            path: "/ws/.unblock".into(),
        };
        assert_eq!(err.code(), ErrorCode::AlreadyInitialized);
        assert_eq!(err.code().exit_code(), 2);
    }

    #[test]
    fn io_maps_to_exit_8() {
        let err = CliError::Io {
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
        };
        assert_eq!(err.code(), ErrorCode::IoError);
        assert_eq!(err.code().exit_code(), 8);
    }

    #[test]
    fn render_forwards_validation_failed() {
        let err = CliError::Render {
            source: unblock_render::RenderError::UnknownFormat {
                name: "xml".to_string(),
            },
        };
        assert_eq!(err.code(), ErrorCode::ValidationFailed);
        assert_eq!(err.code().exit_code(), 4);
    }

    #[cfg(feature = "self-update")]
    #[test]
    fn update_maps_to_internal_error_exit_1() {
        let err = CliError::Update {
            message: "checksum verification failed".to_string(),
        };
        assert_eq!(err.code(), ErrorCode::InternalError);
        assert_eq!(err.code().exit_code(), 1);
    }

    #[test]
    fn to_structured_carries_code_and_message() {
        let err = CliError::AlreadyInitialized {
            path: "/ws/.unblock".into(),
        };
        let structured = to_structured(err);
        assert_eq!(structured.code, ErrorCode::AlreadyInitialized);
        assert!(structured.message.contains("already initialized"));
        assert_eq!(structured.exit_code(), 2);
    }

    #[test]
    fn config_error_is_transparent_and_forwards_code() {
        // A WorkspaceNotFound config error forwards to NotInitialized (exit 2) through the transparent
        // arm — the CLI never re-labels it.
        let cfg = unblock_config::ConfigError::WorkspaceNotFound {
            start: "/ws".into(),
        };
        let err: CliError = cfg.into();
        assert_eq!(err.code(), ErrorCode::NotInitialized);
        assert_eq!(err.code().exit_code(), 2);
    }

    /// D27/AF-4 (non-vacuous): an MCP-server TRANSPORT failure (`McpServerError::Transport`) is an INTERNAL
    /// condition → `InternalError` (exit 1), NEVER a user `IoError` (exit 8). This constructs a REAL
    /// `CliError::Mcp{Transport}` via the `test-util` seam and pins the mapping — mutating the `Mcp`
    /// arm in `code()` to `IoError` turns this RED.
    #[test]
    fn mcp_transport_maps_to_internal_error_exit_1() {
        let err = CliError::Mcp {
            source: unblock_mcp::McpServerError::__transport_error("transport bind failed"),
        };
        assert_eq!(
            err.code(),
            ErrorCode::InternalError,
            "an MCP-server transport failure is INTERNAL, not a user IoError"
        );
        assert_eq!(
            err.code().exit_code(),
            1,
            "InternalError is exit 1 (NOT the IoError exit 8)"
        );
        assert_ne!(
            err.code().exit_code(),
            8,
            "an MCP-server failure must NEVER be the user IoError exit 8"
        );
    }

    /// D27/AF-4 (non-vacuous): an MCP-server RUN-LOOP failure (`McpServerError::RunLoop`) maps identically →
    /// `InternalError` (exit 1). Constructs a REAL `CliError::Mcp{RunLoop}` (an aborted-task `JoinError`)
    /// via the `test-util` seam. Async because building a genuine `JoinError` awaits an aborted task.
    #[tokio::test]
    async fn mcp_run_loop_maps_to_internal_error_exit_1() {
        let err = CliError::Mcp {
            source: unblock_mcp::McpServerError::__run_loop_error().await,
        };
        assert_eq!(err.code(), ErrorCode::InternalError);
        assert_eq!(err.code().exit_code(), 1);
        assert_ne!(
            err.code().exit_code(),
            8,
            "an MCP-server run-loop failure must NEVER be the user IoError exit 8"
        );
    }

    // -- D38 "never swallowed": `emit_diagnostic` actually WRITES the line. --------------------
    //
    // These drive `emit_diagnostic` ITSELF (against a buffer, via its injected sink) rather than a
    // formatter helper, which is what makes them non-vacuous: gutting `emit_diagnostic` to a no-op
    // — the mutation that SURVIVED the entire 54-test surface at the T3.2.1 Verify gate — leaves the
    // buffer empty and turns them RED.

    /// The post-signal / discarded GENUINE error is REPORTED as the human `error[CODE]: message`
    /// line (NFR-14 shape), naming its code and its message. Gutting `emit_diagnostic` → RED.
    #[test]
    fn emit_diagnostic_writes_the_error_line() {
        use super::emit_diagnostic;
        let mut out = Vec::new();
        emit_diagnostic(
            CliError::Mcp {
                source: unblock_mcp::McpServerError::__transport_error("connection reset"),
            },
            &mut out,
        );
        let line = String::from_utf8(out).expect("utf8 diagnostic");
        assert!(
            line.starts_with("error[INTERNAL_ERROR]: "),
            "the D38 diagnostic keeps the NFR-14 `error[CODE]: message` shape, got `{line}`"
        );
        assert!(
            line.contains("connection reset"),
            "the underlying failure is NAMED, never swallowed: `{line}`"
        );
        assert!(
            line.ends_with('\n'),
            "one line, newline-terminated: `{line}`"
        );
    }

    /// The sink is the ONLY output: `emit_diagnostic` decides no exit code and returns nothing — it
    /// forwards each error's OWN code into the line (an `Io` error renders `IO_ERROR`, not a
    /// hard-coded `INTERNAL_ERROR`).
    #[test]
    fn emit_diagnostic_forwards_each_errors_own_code() {
        use super::emit_diagnostic;
        let mut out = Vec::new();
        emit_diagnostic(
            CliError::Io {
                source: std::io::Error::other("libsql close failed"),
            },
            &mut out,
        );
        let line = String::from_utf8(out).expect("utf8 diagnostic");
        assert!(
            line.starts_with("error[IO_ERROR]: "),
            "a teardown IO fault renders its OWN code, got `{line}`"
        );
        assert!(line.contains("libsql close failed"), "`{line}`");
    }

    /// The structured payload for a `CliError::Mcp` renders with the `INTERNAL_ERROR` code + exit 1
    /// (the terminal boundary an MCP-server failure reaches — FR-11 always-valid on error).
    #[test]
    fn mcp_error_to_structured_is_internal_error() {
        let err = CliError::Mcp {
            source: unblock_mcp::McpServerError::__transport_error("boom"),
        };
        let structured = to_structured(err);
        assert_eq!(structured.code, ErrorCode::InternalError);
        assert_eq!(structured.exit_code(), 1);
    }

    // -- D48: the CHANNEL moves, the payload and the exit code do not. ------------------------
    //
    // These drive `into_exit_to` against two buffers, which is the only way to observe the stream
    // CHOICE in-process: `into_exit` writes through `std::io::stdout().lock()` (bypassing libtest
    // capture) and `#![forbid(unsafe_code)]` rules out redirecting the descriptor.
    //
    // What this layer can NEVER see is the CLASSIFIER (`Command::stdout_role`, covered by its own
    // cell in `cli.rs`) or the wrapper's binding of the two real streams — every cell here supplies
    // the role as a literal. A swapped pair of sink arguments in `into_exit` compiles and survives
    // all of them, which is why `tests/mcp_stdout_channel.rs` and the two inverted
    // `tests/mcp_lifecycle.rs` cells are required rather than optional (D48 clause 7).

    use super::{StdoutRole, into_exit_to};
    use unblock_render::OutputFormat;

    /// The provocation these cells share: a CLI-local error with a stable code and exit 2.
    fn already_initialized() -> CliError {
        CliError::AlreadyInitialized {
            path: "/ws/.unblock".into(),
        }
    }

    /// A `SCHEMA_MISMATCH` wrapped exactly as the `mcp` open path produces it — through
    /// `ConfigError`, which is what FORWARDS the hint to the CLI.
    fn schema_mismatch() -> CliError {
        CliError::Config {
            source: unblock_config::ConfigError::MigrationFailed {
                source: unblock_storage::StorageError::SchemaMismatch {
                    found: 99,
                    expected: 2,
                },
            },
        }
    }

    fn parse_payload(bytes: &[u8]) -> serde_json::Value {
        serde_json::from_slice(bytes).unwrap_or_else(|e| {
            panic!(
                "the machine arm must emit ONE valid JSON document: `{}`: {e}",
                String::from_utf8_lossy(bytes)
            )
        })
    }

    /// **U1 — the positive-landing cell.** For a PROTOCOL-channel command the `json` document lands
    /// on STDERR and stdout is left untouched. The POSITIVE half (stderr parses and carries the
    /// code) is what survives a delete-the-diagnostic mutation; the stdout-empty half alone would
    /// not, since stdout is legitimately empty on every pre-run-loop failure.
    #[test]
    fn a_machine_error_for_a_protocol_role_lands_on_stderr() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = into_exit_to(
            already_initialized(),
            OutputFormat::Json,
            StdoutRole::Protocol,
            &mut out,
            &mut err,
        );
        assert!(
            out.is_empty(),
            "D48: nothing may reach the framing channel, got `{}`",
            String::from_utf8_lossy(&out)
        );
        let payload = parse_payload(&err);
        assert_eq!(payload["code"], "ALREADY_INITIALIZED");
        assert_eq!(payload["retryable"], false);
        assert!(
            !String::from_utf8_lossy(&err).starts_with("error["),
            "the FULL document moves, never the degraded human line"
        );
        assert_eq!(code, 2, "the channel moved; the exit code did not");
    }

    /// **U2 — the `Reports` mirror.** The in-process guard that no OTHER command's bytes move: a
    /// renderer ignoring the classification and writing everything to stderr turns this RED. It
    /// says nothing about the classifier itself, which hands it a literal.
    #[test]
    fn a_machine_error_for_a_reports_role_still_lands_on_stdout() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = into_exit_to(
            already_initialized(),
            OutputFormat::Json,
            StdoutRole::Reports,
            &mut out,
            &mut err,
        );
        let payload = parse_payload(&out);
        assert_eq!(payload["code"], "ALREADY_INITIALIZED");
        assert!(
            err.is_empty(),
            "a REPORTS command is byte-unchanged by D48, got stderr `{}`",
            String::from_utf8_lossy(&err)
        );
        assert_eq!(code, 2);
    }

    /// **U3** — `robot` moves too. Kills a carve-out keyed on `Json` alone.
    #[test]
    fn robot_moves_too_not_only_json() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = into_exit_to(
            already_initialized(),
            OutputFormat::Robot,
            StdoutRole::Protocol,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 2);
        assert!(
            out.is_empty(),
            "robot is a MACHINE format: it moves as well"
        );
        let payload = parse_payload(&err);
        assert_eq!(payload["code"], "ALREADY_INITIALIZED");
    }

    /// **U4** — the human arm is role-INDEPENDENT: `plain` writes its one line to stderr under both
    /// classifications and never to stdout.
    #[test]
    fn the_human_arm_is_unchanged_for_both_roles() {
        for role in [StdoutRole::Reports, StdoutRole::Protocol] {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = into_exit_to(
                already_initialized(),
                OutputFormat::Plain,
                role,
                &mut out,
                &mut err,
            );
            assert!(out.is_empty(), "{role:?}: the human arm never uses stdout");
            let line = String::from_utf8(err).expect("utf8 diagnostic");
            assert!(
                line.starts_with("error[ALREADY_INITIALIZED]: "),
                "{role:?}: the NFR-14 line shape is untouched, got `{line}`"
            );
            assert!(line.ends_with('\n'), "{role:?}: `{line}`");
            assert_eq!(code, 2);
        }
    }

    /// **U5 — D48's payload clause in full.** The relocated document is NOT degraded: the `hint`,
    /// which is the actionable half of the D46 mixed-version case, survives the move. Degrading the
    /// protocol arm to the `error[CODE]` line (which drops it) turns this RED.
    #[test]
    fn the_hint_survives_the_move_to_stderr() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = into_exit_to(
            schema_mismatch(),
            OutputFormat::Json,
            StdoutRole::Protocol,
            &mut out,
            &mut err,
        );
        assert!(out.is_empty());
        let payload = parse_payload(&err);
        assert_eq!(payload["code"], "SCHEMA_MISMATCH");
        let hint = payload["hint"].as_str().unwrap_or_default();
        assert!(
            hint.contains("NEWER") && hint.contains("unblock update"),
            "the operator's next action rides in the `hint`, and it must not be dropped: {payload}"
        );
        assert_eq!(code, 2);
    }

    /// **U6 — the exit code does not move (D48 clause 4), asserted at the choke point.** Every
    /// combination of role and format returns the error's OWN 0–8 code. A protocol arm returning a
    /// fixed code turns this RED.
    #[test]
    fn the_channel_move_never_moves_the_exit_code() {
        let cases: [(fn() -> CliError, u8); 2] = [
            (
                || CliError::Mcp {
                    source: unblock_mcp::McpServerError::__transport_error("boom"),
                },
                1,
            ),
            (
                || CliError::Config {
                    source: unblock_config::ConfigError::WorkspaceNotFound {
                        start: "/ws".into(),
                    },
                },
                2,
            ),
        ];
        for (make, expected) in cases {
            for fmt in [OutputFormat::Json, OutputFormat::Plain] {
                for role in [StdoutRole::Reports, StdoutRole::Protocol] {
                    let (mut out, mut err) = (Vec::new(), Vec::new());
                    let code = into_exit_to(make(), fmt, role, &mut out, &mut err);
                    assert_eq!(
                        code, expected,
                        "D48 moves the CHANNEL and nothing else ({fmt:?}, {role:?})"
                    );
                }
            }
        }
    }

    /// **U8 — "byte for byte", ASSERTED rather than adjectival.** The same error at the same format
    /// renders identical BYTES whichever channel it lands on; every other cell checks MEMBERS, so a
    /// mutation that reformatted or reordered the document would pass them all. Injecting both
    /// sinks is what makes this two comparisons instead of a spawned process.
    ///
    /// **The equality is RELATIVE, and that limit is stated rather than left to be assumed:** it
    /// pins the two channels against EACH OTHER, not either of them against the bytes that shipped
    /// before D48. A mutation that reformatted BOTH arms identically would survive this cell. What
    /// closes that chain is the shipped end-to-end cells which still assert a rendered payload on
    /// STDOUT for the OTHER commands — they are the anchor to today's bytes. They are ENUMERATED
    /// (never counted, and never summarised as a number of commands) in `tests/mcp_stdout_channel.rs`'s
    /// module doc, each with the command it drives; that enumeration was measured by applying the
    /// blanket-`Protocol` classifier mutation and reading the failures off. Between them they cover
    /// every `Reports` command except `version`, which by design pins nothing here (clause 6(iii)).
    #[test]
    fn the_relocated_payload_is_byte_identical() {
        for fmt in [OutputFormat::Json, OutputFormat::Robot] {
            let (mut reports_out, mut reports_err) = (Vec::new(), Vec::new());
            let reports_code = into_exit_to(
                already_initialized(),
                fmt,
                StdoutRole::Reports,
                &mut reports_out,
                &mut reports_err,
            );
            let (mut protocol_out, mut protocol_err) = (Vec::new(), Vec::new());
            let protocol_code = into_exit_to(
                already_initialized(),
                fmt,
                StdoutRole::Protocol,
                &mut protocol_out,
                &mut protocol_err,
            );
            assert_eq!(
                reports_code, protocol_code,
                "{fmt:?}: same input, same code"
            );
            assert!(!reports_out.is_empty(), "{fmt:?}: nothing was rendered");
            assert_eq!(
                reports_out, protocol_err,
                "{fmt:?}: the SAME document, byte for byte — only the stream differs (D48 clause 3)"
            );
            assert!(reports_err.is_empty() && protocol_out.is_empty(), "{fmt:?}");
        }
    }

    /// **U9 — the payload is NEWLINE-TERMINATED, on whichever channel it lands.** The one thing the
    /// relative comparison in U8 structurally cannot see: `write_payload` is a SINGLE site, so
    /// dropping its trailing `write_all(b"\n")` mutates both of U8's operands identically and that
    /// cell stays green while every payload in the product loses its terminator.
    ///
    /// It is a behavioural loss and not a cosmetic one, on BOTH roles at once — which is why both
    /// are driven here rather than only the relocated one. The renderer emits
    /// `serde_json::to_string`, i.e. no trailing newline of its own; a host reading the `mcp`
    /// child's stderr line-wise (the only place a D48-relocated failure can now be read) and a
    /// shell pipeline reading a `Reports` command's stdout would both see the document only at EOF.
    ///
    /// EXACTLY one newline is asserted, not merely a trailing one: the compact single-line render is
    /// the premise the shared harness oracle rests on (`tests/common/mod.rs`
    /// `structured_error_on_stderr` locates the payload as ONE line), so a second terminator, or a
    /// switch to pretty JSON, is a defect here rather than a harmless reformat.
    #[test]
    fn the_payload_is_newline_terminated_on_both_channels() {
        for (role, expect_on_stdout) in [(StdoutRole::Reports, true), (StdoutRole::Protocol, false)]
        {
            for fmt in [OutputFormat::Json, OutputFormat::Robot] {
                let (mut out, mut err) = (Vec::new(), Vec::new());
                let code = into_exit_to(already_initialized(), fmt, role, &mut out, &mut err);
                assert_eq!(code, 2, "{role:?}/{fmt:?}");
                let written = if expect_on_stdout { &out } else { &err };
                let text = String::from_utf8(written.clone()).expect("utf8 payload");
                assert!(
                    text.ends_with('\n'),
                    "{role:?}/{fmt:?}: the payload must be TERMINATED — a line-wise consumer \
                     (an MCP host tailing stderr, a shell pipeline reading stdout) sees an \
                     unterminated document only at EOF: `{text}`"
                );
                assert_eq!(
                    text.matches('\n').count(),
                    1,
                    "{role:?}/{fmt:?}: ONE compact line plus ONE terminator — the premise the \
                     harness oracle's line-wise search rests on: `{text}`"
                );
                // And the terminator is the ONLY thing added: the line still parses whole.
                let payload = parse_payload(text.trim_end().as_bytes());
                assert_eq!(payload["code"], "ALREADY_INITIALIZED");
            }
        }
    }
}
