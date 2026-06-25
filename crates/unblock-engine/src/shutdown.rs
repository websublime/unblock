//! Cooperative shutdown **flag-read** surface (FR-17, OQ-4).
//!
//! The OS signal-handler install (SIGINT/SIGTERM/SIGHUP → an atomic flag, via `signal-hook`) lives
//! in **`unblock-cli`** (L7): a library must not hijack process-global signals (OQ-4 RESOLVED). The
//! engine only **reads** the flag at mutation checkpoints (see `permit::acquire_write` and
//! `write.rs`). The cli wires the `Arc<AtomicBool>` it sets into the `Session` via
//! [`crate::Session::with_shutdown_flag`].
//!
//! This module exposes the read view. The flag a `Session` carries is the one the cli installs;
//! when no flag is wired (e.g. tests, the mcp library default) the `Session` owns its own
//! never-set flag, so [`crate::Session::is_shutdown_requested`] is `false` until something flips it.

use std::sync::atomic::{AtomicBool, Ordering};

/// Read whether a cooperative shutdown has been requested on `flag`.
///
/// A thin, side-effect-free read of the cli-installed flag (FR-17). The engine never *sets* the
/// flag (only `shutdown()` flips a `Session`'s own flag); the OS handler that sets a wired flag
/// lives in `unblock-cli` (OQ-4).
#[must_use]
pub fn is_shutdown_requested(flag: &AtomicBool) -> bool {
    flag.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::is_shutdown_requested;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn reads_the_wired_flag() {
        let flag = AtomicBool::new(false);
        assert!(!is_shutdown_requested(&flag));
        flag.store(true, Ordering::SeqCst);
        assert!(is_shutdown_requested(&flag));
    }
}
