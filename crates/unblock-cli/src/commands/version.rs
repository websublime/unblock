//! `unblock version` (D27/AD-5) — emit version/build metadata from `build.rs`-provided `option_env!`
//! values. Runs with NO workspace, NO network, NO git (NFR-6/D13; the update-check lives only in
//! `unblock update`). Rendered via the shared to-`DiagnosticReport` path.

use crate::cli::{GlobalArgs, VersionArgs};
use crate::exit::CliError;
use crate::output::{self, ToDiagnosticReport, VersionReport};

/// Run `unblock version`.
///
/// `--short` prints the bare `CARGO_PKG_VERSION` and returns. Otherwise builds a `VersionReport` from
/// compile-time env (`option_env!("UNBLOCK_BUILD_*")`, absent → `None`) and renders it in the format
/// resolved from CLI+env (no workspace → `--output > UNBLOCK_OUTPUT_FORMAT > Json`, FR-13/SF-1).
///
/// # Errors
/// - [`CliError::Render`]/[`CliError::Io`] if rendering / writing the report fails.
pub fn run(args: &VersionArgs, global: &GlobalArgs) -> Result<Option<u8>, CliError> {
    if args.short {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(None);
    }

    let report = VersionReport {
        version: env!("CARGO_PKG_VERSION").to_string(),
        // `build.rs` always emits the profile; default to "release" if somehow absent.
        build: option_env!("UNBLOCK_BUILD_PROFILE")
            .unwrap_or("release")
            .to_string(),
        commit: non_empty(option_env!("UNBLOCK_BUILD_COMMIT")),
        rustc: non_empty(option_env!("UNBLOCK_BUILD_RUSTC")),
        target: non_empty(option_env!("UNBLOCK_BUILD_TARGET")),
        features: enabled_features(),
    };

    let fmt = output::pick_cli_format(global);
    output::emit_report(&report.to_report(), fmt).map(|()| None)
}

/// Map an `option_env!` result to `Some(String)` only when present AND non-empty.
fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// The Cargo features enabled in this build (bd-fidelity `features` field).
fn enabled_features() -> Vec<String> {
    let mut features = Vec::new();
    if cfg!(feature = "self-update") {
        features.push("self-update".to_string());
    }
    features
}

#[cfg(test)]
mod tests {
    use super::{enabled_features, non_empty};

    #[test]
    fn non_empty_filters_blank_and_absent() {
        assert_eq!(non_empty(None), None);
        assert_eq!(non_empty(Some("")), None);
        assert_eq!(non_empty(Some("   ")), None);
        assert_eq!(non_empty(Some("abc123")), Some("abc123".to_string()));
    }

    #[test]
    fn enabled_features_reflect_cfg() {
        let features = enabled_features();
        // Default build has `self-update`; `--no-default-features` drops it. Assert consistency with cfg.
        assert_eq!(
            features.contains(&"self-update".to_string()),
            cfg!(feature = "self-update")
        );
    }
}
