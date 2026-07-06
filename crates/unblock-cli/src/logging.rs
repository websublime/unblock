//! Cli-local `tracing-subscriber` init — STDERR-ONLY (NFR-14) with a `-v/-q` level map.
//!
//! The CLI OWNS its subscriber (rather than delegating to engine's `init_tracing`) so it can
//! GUARANTEE a stderr writer at the process edge: a `serve` run must never let a log line hit STDOUT,
//! which carries the MCP framing (a single stray line corrupts the protocol). The reliability target
//! is referenced from the engine's re-exported [`RELIABILITY_TARGET`] const so the target name stays a
//! single SSOT (NFR-13). Idempotent via `try_init().ok()` (a second call — e.g. in tests — is a no-op).

use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;
use unblock_engine::RELIABILITY_TARGET;

/// Map `-v`/`-q` to a level filter (NFR-13):
/// - `-q` → ERROR only (overrides `-v`);
/// - default → WARN;
/// - `-v` → INFO; `-vv` → DEBUG; `-vvv+` → TRACE.
#[must_use]
fn level_for(verbose: u8, quiet: bool) -> LevelFilter {
    if quiet {
        return LevelFilter::ERROR;
    }
    match verbose {
        0 => LevelFilter::WARN,
        1 => LevelFilter::INFO,
        2 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    }
}

/// Initialize stderr-only tracing at the level implied by `-v`/`-q`.
///
/// Best-effort + idempotent: if a global subscriber is already installed (a repeated call, a test),
/// this is a no-op. `RUST_LOG` is honored when set (it takes precedence over the flag-derived
/// directive); otherwise the reliability target is filtered at the mapped level.
pub fn init_logging(verbose: u8, quiet: bool) {
    let level = level_for(verbose, quiet);
    // Default directive: the mapped level as the global floor + the reliability target at that level.
    // `RUST_LOG` (if set) overrides. An unparseable directive falls back to the mapped level.
    let directive = format!("{level},{RELIABILITY_TARGET}={level}");
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&directive))
        .unwrap_or_else(|_| EnvFilter::new("warn"));

    let _already_set = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr) // NFR-14: diagnostics → stderr, NEVER stdout.
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::{init_logging, level_for};
    use tracing::level_filters::LevelFilter;

    #[test]
    fn quiet_maps_to_error_and_overrides_verbose() {
        assert_eq!(level_for(3, true), LevelFilter::ERROR);
    }

    #[test]
    fn verbose_count_maps_to_expected_levels() {
        assert_eq!(level_for(0, false), LevelFilter::WARN);
        assert_eq!(level_for(1, false), LevelFilter::INFO);
        assert_eq!(level_for(2, false), LevelFilter::DEBUG);
        assert_eq!(level_for(9, false), LevelFilter::TRACE);
    }

    #[test]
    fn init_is_idempotent() {
        // A second call must not panic (try_init no-ops when a global subscriber already exists).
        init_logging(0, false);
        init_logging(2, false);
    }
}
