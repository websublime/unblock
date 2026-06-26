//! `tracing` init helper on the `unblock.reliability` target (NFR-13).
//!
//! The engine emits spans/events on the `unblock.reliability` target — INFO for guard activations /
//! external-path use / conflict-marker rejection / force overrides; DEBUG per-file/per-issue. The
//! structured-stdout / diagnostics-stderr discipline (NFR-14) is the renderer's job; the engine only
//! emits.
//!
//! [`init_tracing`] is a best-effort, idempotent helper a binary may call once at start-up. It
//! installs a process-global subscriber only if none is set yet (it never panics on a double-call,
//! and never errors out a library caller that already installed its own subscriber).

/// The `tracing` target the engine's reliability spans/events are emitted on (NFR-13).
pub const RELIABILITY_TARGET: &str = "unblock.reliability";

/// Options controlling [`init_tracing`].
#[derive(Debug, Clone, Copy)]
pub struct TracingOptions {
    /// The maximum verbosity to emit (INFO for guard activations, DEBUG for per-issue detail).
    pub level: tracing::Level,
}

impl Default for TracingOptions {
    /// INFO-level by default (guard activations / external-path use, NFR-13).
    fn default() -> Self {
        Self {
            level: tracing::Level::INFO,
        }
    }
}

/// Initialize `tracing` for the engine's `unblock.reliability` target (NFR-13).
///
/// Best-effort and idempotent: it tries to install a process-global subscriber at the requested
/// level and returns quietly if one is already set (so a binary that wired its own subscriber, or a
/// repeated call in tests, never panics or errors). Diagnostics go to stderr (NFR-14); structured
/// output to stdout is the renderer's concern, not this helper's.
///
/// Exposing a subscriber-install helper from a **library** is intentional-but-best-effort: it is
/// opt-in (a caller must call it), idempotent, and override-safe (`try_init` no-ops when a global
/// subscriber already exists), so it never hijacks process state the way a global OS signal handler
/// would (contrast the cli-owned shutdown handler, OQ-4). The cli's install is the canonical one.
pub fn init_tracing(opts: TracingOptions) {
    use tracing_subscriber::EnvFilter;

    // Default the filter to the requested level on the reliability target; honour RUST_LOG if set.
    let directive = format!("{}={}", RELIABILITY_TARGET, level_str(opts.level));
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&directive))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    // `try_init` returns Err if a global subscriber is already set — that is fine (idempotent).
    let _already_set = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr) // diagnostics -> stderr (NFR-14).
        .try_init();
}

/// The lowercase `tracing` level string for an `EnvFilter` directive.
const fn level_str(level: tracing::Level) -> &'static str {
    match level {
        tracing::Level::TRACE => "trace",
        tracing::Level::DEBUG => "debug",
        tracing::Level::INFO => "info",
        tracing::Level::WARN => "warn",
        tracing::Level::ERROR => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::{RELIABILITY_TARGET, TracingOptions, init_tracing, level_str};

    #[test]
    fn target_is_pinned() {
        assert_eq!(RELIABILITY_TARGET, "unblock.reliability");
    }

    #[test]
    fn default_options_are_info() {
        assert_eq!(TracingOptions::default().level, tracing::Level::INFO);
    }

    #[test]
    fn level_strings_are_lowercase() {
        assert_eq!(level_str(tracing::Level::INFO), "info");
        assert_eq!(level_str(tracing::Level::DEBUG), "debug");
    }

    #[test]
    fn init_is_idempotent_and_never_panics() {
        // Calling twice must not panic (the second call is a no-op when a subscriber exists).
        init_tracing(TracingOptions::default());
        init_tracing(TracingOptions::default());
    }
}
