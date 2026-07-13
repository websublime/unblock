//! Cooperative-shutdown signal install (FR-17, D27/AD-4) — the CLI owns process-global signal
//! handling (a library must not hijack it, OQ-4).
//!
//! [`install`] returns a [`ShutdownHandle`] bundling:
//! - a `tokio_util::sync::CancellationToken` (`token`) fed into `McpServerOptions.cancel` — a `cancel()`
//!   drives `unblock_mcp::run_mcp_server` to return `Ok`;
//! - an `Arc<AtomicBool>` (`flag`) wired into the engine via `Session::with_shutdown_flag` — the
//!   engine reads it at mutation checkpoints (`Session::is_shutdown_requested`);
//! - an `Arc<AtomicU8>` (`signalled`) recording `128 + signo` on the FIRST signal so `run_mcp_server` can
//!   yield the conventional signal exit code.
//!
//! On unix (`#[cfg(unix)]`) a dedicated NORMAL thread runs `signal_hook::iterator::Signals` for
//! SIGINT/SIGTERM/SIGHUP. **First signal:** CAS `signalled` 0 → `128+signo`, then fire BOTH sinks
//! (`token.cancel()` + `flag.store(true)`). **Second signal:** `std::process::exit(128+signo)` — the
//! async-signal-safe hard exit (signal-hook turns the raw signal into a channel event on a normal
//! thread, so `process::exit` is fine there; NO `libc::_exit`, `#![forbid(unsafe_code)]` holds).
//!
//! On non-unix (`#[cfg(not(unix))]`) [`install`] returns a fresh handle with NO handler (a no-op);
//! the MCP server still shuts down on EOF (NFR-11).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use tokio_util::sync::CancellationToken;

/// The cooperative-shutdown handle handed to `run_mcp_server` (D27/AD-4).
#[derive(Debug, Clone)]
pub struct ShutdownHandle {
    /// The cancellation token fed to `McpServerOptions.cancel` (a `cancel()` returns `run_mcp_server` cleanly).
    pub token: CancellationToken,
    /// The engine shutdown flag wired via `Session::with_shutdown_flag`.
    pub flag: Arc<AtomicBool>,
    /// The recorded `128 + signo` on the first signal (`0` = none yet).
    pub signalled: Arc<AtomicU8>,
}

impl ShutdownHandle {
    /// The exit code the process should return after `run_mcp_server` returns, or `None` on a clean EOF exit.
    ///
    /// Returns `Some(128 + signo)` when a signal drove the shutdown, `None` otherwise (exit 0).
    #[must_use]
    pub fn signal_exit_code(&self) -> Option<u8> {
        match self.signalled.load(Ordering::SeqCst) {
            0 => None,
            code => Some(code),
        }
    }
}

/// Install the cooperative-shutdown handlers and return the handle (unix). See the module docs.
#[cfg(unix)]
#[must_use]
pub fn install() -> ShutdownHandle {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    use signal_hook::iterator::Signals;

    let handle = ShutdownHandle {
        token: CancellationToken::new(),
        flag: Arc::new(AtomicBool::new(false)),
        signalled: Arc::new(AtomicU8::new(0)),
    };

    // Registering may fail (e.g. an already-reserved signal) — if so we degrade to EOF-only shutdown
    // (still correct: `run_mcp_server` returns on stdin EOF). We do not panic at the process edge.
    let Ok(mut signals) = Signals::new([SIGINT, SIGTERM, SIGHUP]) else {
        return handle;
    };

    let token = handle.token.clone();
    let flag = handle.flag.clone();
    let signalled = handle.signalled.clone();

    // A dedicated NORMAL OS thread drains the signal iterator (NOT a raw async-signal handler), so
    // `token.cancel()` / `process::exit` are safe to call here.
    std::thread::spawn(move || {
        for signo in signals.forever() {
            // `128 + signo` fits u8 for the standard signals (SIGHUP=1, SIGINT=2, SIGTERM=15).
            let code = 128u8.saturating_add(u8::try_from(signo).unwrap_or(0));
            // First signal: record + fire both sinks. `compare_exchange` from 0 succeeds exactly once.
            if signalled
                .compare_exchange(0, code, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                token.cancel();
                flag.store(true, Ordering::SeqCst);
            } else {
                // Second (or later) signal: hard, async-signal-safe exit (normal thread → safe).
                let recorded = signalled.load(Ordering::SeqCst);
                std::process::exit(i32::from(recorded));
            }
        }
    });

    handle
}

/// Install a no-op handle (non-unix) — the MCP server shuts down on EOF (NFR-11).
#[cfg(not(unix))]
#[must_use]
pub fn install() -> ShutdownHandle {
    ShutdownHandle {
        token: CancellationToken::new(),
        flag: Arc::new(AtomicBool::new(false)),
        signalled: Arc::new(AtomicU8::new(0)),
    }
}

#[cfg(test)]
mod tests {
    use super::ShutdownHandle;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    use tokio_util::sync::CancellationToken;

    fn handle() -> ShutdownHandle {
        ShutdownHandle {
            token: CancellationToken::new(),
            flag: Arc::new(AtomicBool::new(false)),
            signalled: Arc::new(AtomicU8::new(0)),
        }
    }

    #[test]
    fn signal_exit_code_is_none_before_any_signal() {
        assert_eq!(handle().signal_exit_code(), None);
    }

    #[test]
    fn signal_exit_code_reports_128_plus_signo() {
        let h = handle();
        // Simulate the first-signal recording: SIGTERM(15) → 143.
        h.signalled.store(143, Ordering::SeqCst);
        assert_eq!(h.signal_exit_code(), Some(143));
        // SIGINT(2) → 130.
        h.signalled.store(130, Ordering::SeqCst);
        assert_eq!(h.signal_exit_code(), Some(130));
    }

    #[test]
    fn first_signal_semantics_fire_both_sinks() {
        // Model the install() first-signal branch: CAS 0 -> code, then cancel + set flag.
        let h = handle();
        let code = 128u8 + 15; // SIGTERM.
        assert!(
            h.signalled
                .compare_exchange(0, code, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        );
        h.token.cancel();
        h.flag.store(true, Ordering::SeqCst);
        assert!(h.token.is_cancelled());
        assert!(h.flag.load(Ordering::SeqCst));
        assert_eq!(h.signal_exit_code(), Some(143));
        // A second CAS from 0 fails (idempotent first-signal capture).
        assert!(
            h.signalled
                .compare_exchange(0, 130, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn install_returns_a_usable_handle() {
        let h = super::install();
        assert!(!h.token.is_cancelled());
        assert!(!h.flag.load(Ordering::SeqCst));
        assert_eq!(h.signal_exit_code(), None);
    }
}
