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
//! **NFR-14 + FR-11 stream split:** in `json`/`robot` the structured error renders to STDOUT (always
//! valid JSON even on error, FR-11); in `plain`/`csv`/`markdown` a human `error[CODE]: message` line
//! goes to STDERR (diagnostics, NFR-14).

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use snafu::Snafu;
use unblock_error::{ErrorCode, StructuredError};
use unblock_render::{OutputFormat, RenderOptions, renderer_for};

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
/// - `json`/`robot`: the `StructuredError` renders to STDOUT (always valid JSON even on error);
/// - `plain`/`csv`/`markdown`: a human `error[CODE]: message` line goes to STDERR (diagnostics).
///
/// The 0–8 cast is CLI-owned: `ExitCode::from(structured.exit_code())`.
#[must_use]
pub(crate) fn into_exit(err: CliError, fmt: OutputFormat) -> ExitCode {
    let structured = to_structured(err);
    let exit = structured.exit_code();

    match fmt {
        // Machine formats: the structured payload to STDOUT (FR-11 always-valid JSON on error).
        OutputFormat::Json | OutputFormat::Robot => {
            let opts = RenderOptions::default();
            if let Ok(out) = renderer_for(fmt, opts.clone()).structured_error(&structured, &opts) {
                let mut stdout = std::io::stdout().lock();
                let _ignored = stdout.write_all(out.stdout.as_bytes());
                let _ignored = stdout.write_all(b"\n");
            } else {
                // Rendering the error itself failed — still surface something machine-safe on stderr.
                emit_human(&structured);
            }
        }
        // Human formats: a one-line diagnostic to STDERR (NFR-14).
        _ => emit_human(&structured),
    }

    ExitCode::from(exit)
}

/// Report a `CliError` as a human `error[CODE]: message` line on `out` **without deciding the exit
/// code** (D38, spine §5b) — the `mcp` command's GENUINE-error diagnostic sink.
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
/// framing ONLY (NFR-14), and FR-11's always-valid-JSON-on-stdout rule binds only the UNSIGNALLED
/// `Err` path (which renders via [`into_exit`], not here).
pub(crate) fn emit_diagnostic(err: CliError, out: &mut impl Write) {
    // A diagnostic must never itself become a failure: a closed/failing stderr is not a reason to
    // change the exit code we were asked to deliver.
    let _ignored = write_human(&to_structured(err), out);
}

/// Write a human `error[CODE]: message` line to STDERR (NFR-14).
fn emit_human(structured: &StructuredError) {
    let _ignored = write_human(structured, &mut std::io::stderr().lock());
}

/// Render the single human diagnostic line shape (`error[CODE]: message`) onto `out` — the ONE
/// place that shape exists, shared by [`into_exit`]'s human arm and [`emit_diagnostic`].
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
}
