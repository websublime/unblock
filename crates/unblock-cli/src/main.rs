//! `unblock` — process entry point for the lifecycle/ops CLI. OWNS the tokio runtime and delegates
//! to the library facade [`unblock_cli::run`], returning its [`std::process::ExitCode`]. All routing,
//! logging, dispatch, and the 0–8 exit-code boundary live in the library so they stay hermetically
//! testable. See `docs/plans/crates/unblock-cli.md` and the spine §5b (`mcp`).
//!
//! **Why the runtime is built by hand instead of `#[tokio::main]` (D38 no-hang invariant, spine
//! §5b).** `unblock mcp` binds `tokio::io::stdin()`, whose read runs as a BLOCKING-pool task that is
//! not cancellable. When the MCP run loop returns while that read is still parked on an open stdin
//! (every cancellation-driven shutdown, and any error return that is not preceded by EOF),
//! `Runtime::drop` → `BlockingPool::shutdown` blocks FOREVER waiting for it. `#[tokio::main]` expands
//! the runtime to a temporary, so that blocking drop is structurally unavoidable — which is exactly
//! how a SIGTERM delivered before a client's `initialize` handshake used to hang the process forever
//! (PRD §4/D38). Owning the runtime lets us dispose of it with [`Runtime::shutdown_background`],
//! which CONSUMES it (so no `Drop` runs and nothing can block), on EVERY return path — `Ok`, `Err`,
//! and the `128+signo` signal exit alike.
//!
//! Nothing load-bearing is lost by not joining the pool: rmcp flushes its framing per-message before
//! returning, `session.shutdown()` closes libsql INSIDE `block_on`, tracing writes to
//! `std::io::stderr` with no `non_blocking` guard to flush, and `exit.rs` renders synchronously.
#![forbid(unsafe_code)]

use std::process::ExitCode;

use unblock_error::{ErrorCode, StructuredError};

fn main() -> ExitCode {
    // `worker_threads` is deliberately omitted — the default (one per core) is what `#[tokio::main]`
    // used, so this change is runtime-ownership only, NOT a scheduling change.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(source) => return runtime_build_failure(&source),
    };

    let code = runtime.block_on(unblock_cli::run());

    // D38: dispose NON-BLOCKINGLY. `shutdown_background` consumes the runtime, so its blocking `Drop`
    // never runs and a still-parked `tokio::io::stdin()` read can no longer wedge the exit. This must
    // stay AFTER `block_on` (which already ran every await to completion, including the clean libsql
    // close) and BEFORE returning the code.
    runtime.shutdown_background();

    code
}

/// Map a tokio runtime-build failure to the 0–8 table (spine §2.3). This is the process edge, before
/// the library facade exists, so it renders here rather than through `exit.rs`: a runtime that cannot
/// be built is an INTERNAL condition (exit 1), not a user I/O fault. No `unwrap`/`expect`/`panic!` —
/// the edge stays honest even when tokio itself is unavailable.
fn runtime_build_failure(source: &std::io::Error) -> ExitCode {
    let structured = StructuredError::from_code(
        ErrorCode::InternalError,
        format!("failed to build the tokio runtime: {source}"),
    );
    // stderr, per NFR-14: with no runtime there is no command whose stdout contract could apply.
    eprintln!(
        "error[{}]: {}",
        structured.code.as_str(),
        structured.message
    );
    ExitCode::from(ErrorCode::InternalError.exit_code())
}
