//! `unblock doctor` — the read-only health report.
//!
//! At **T3.3 (HEALTH-LITE, D29/F4)** this command ROUTES through the now-wired `Session::doctor()` for
//! OUTPUT (integrity + pure file-state anomalies, mapped onto `DiagnosticReport { kind: Info, .. }`
//! reusing `DiagnosticKind::Info` — F2, rendered in all five formats via `Renderer::diagnostics`),
//! while DERIVING its exit code from a SEPARATE, auxiliary `Session::integrity_check()` read (F4
//! mechanism = option (a), orchestrator-pinned) so the mutation-proven `doctor_exit(&integrity:
//! &[String])` stays BYTE-IDENTICAL. **Non-zero exit only on detected corruption:** a non-empty
//! `integrity_check` → exit 2 (`ErrorCode::DatabaseError`); Lint/file-state/orphan findings are
//! ADVISORY (reported, NO exit flip); else exit 0. The exit MUST NOT be derived by string-matching the
//! flattened `Info` findings. `--repair` + the full Healthy/Drifted/Recoverable/Unsafe taxonomy land at
//! **v1.1** over the `doctor()`/`recover()` seam.

use unblock_config::{CliOverrides, open_with_storage_with_cli};
use unblock_engine::{Session, SessionConfig};
use unblock_error::ErrorCode;

use crate::cli::DoctorArgs;
use crate::exit::CliError;
use crate::output;

/// Run `unblock doctor` (health-lite).
///
/// # Errors
/// - [`CliError::Config`] if the workspace cannot be opened;
/// - [`CliError::Engine`] if the session cannot be opened or the doctor / integrity read fails;
/// - [`CliError::Render`]/[`CliError::Io`] if rendering / writing the report fails.
pub async fn run(_args: &DoctorArgs, overrides: &CliOverrides) -> Result<Option<u8>, CliError> {
    let ctx = open_with_storage_with_cli(overrides).await?;
    let fmt = ctx.config.output_format;

    let session = Session::open(
        ctx,
        SessionConfig {
            import_on_open: false,
            ..SessionConfig::default()
        },
    )
    .await?;

    // OUTPUT: the wired `Session::doctor()` report (integrity + file-state anomalies), reusing
    // `DiagnosticKind::Info` (F2) — rendered directly, all five formats (F4).
    let report = session.doctor().await?;
    // EXIT: a SEPARATE `integrity_check()` read feeds the byte-identical `doctor_exit` (F4/MF1) — the
    // exit is NEVER derived by string-matching the rendered findings.
    let integrity_problems = session.integrity_check().await?;
    let exit = doctor_exit(&integrity_problems);

    output::emit_report(&report, fmt)?;

    // The report was already emitted (it carries the integrity + file-state findings); `doctor_exit`
    // derives the non-zero exit ONLY on detected corruption (AF-1/F4) — file-state / Lint findings stay
    // advisory (they never flip the exit).
    Ok(exit)
}

/// Derive the doctor exit code from the integrity probe (D27/AF-1; preserved BYTE-IDENTICAL at T3.3 per
/// F4/MF1 — the exit derivation is decoupled from the rendered `doctor()` report). PURE: a NON-EMPTY
/// `integrity` (detected corruption) flips to `Some(ErrorCode::DatabaseError.exit_code())` (exit 2, the
/// db bucket — §2.3 SSOT, no new code); an empty probe is `None` (exit 0). File-state / Lint / orphan
/// findings are ADVISORY — they NEVER reach this function and NEVER flip the exit.
fn doctor_exit(integrity: &[String]) -> Option<u8> {
    if integrity.is_empty() {
        None
    } else {
        Some(ErrorCode::DatabaseError.exit_code())
    }
}

#[cfg(test)]
mod tests {
    use super::doctor_exit;

    #[test]
    fn database_error_exit_is_two() {
        use unblock_error::ErrorCode;
        assert_eq!(ErrorCode::DatabaseError.exit_code(), 2);
    }

    /// AF-1/F4 flip (non-vacuous): a NON-EMPTY integrity probe → `Some(2)` (the db bucket). This
    /// exercises the exact `doctor_exit` non-empty branch that `run` delegates to — mutating that
    /// branch (to `None` or a wrong code) turns this RED.
    #[test]
    fn doctor_exit_non_empty_integrity_is_exit_2() {
        let problems = vec!["*** in database main *** page 3 is never used".to_string()];
        assert_eq!(
            doctor_exit(&problems),
            Some(2),
            "a non-empty integrity_check flips the exit to the db bucket (exit 2)"
        );
    }

    /// AF-1/F4: a clean (empty) integrity probe → `None` (exit 0). The healthy leg of the same branch;
    /// since file-state/Lint findings never reach `doctor_exit`, a clean integrity is always exit 0
    /// regardless of any advisory anomaly in the rendered report.
    #[test]
    fn doctor_exit_empty_integrity_is_none() {
        assert_eq!(
            doctor_exit(&[]),
            None,
            "a clean integrity_check does not flip the exit (advisory findings never flip it)"
        );
    }
}
