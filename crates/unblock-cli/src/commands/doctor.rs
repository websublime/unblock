//! `unblock doctor` (D27/AF-1 — doctor-LITE) — a read-only health report composed from the BUILD-now
//! diagnostics reads + the integrity probe.
//!
//! Opens the `Session` and composes `diagnostics(Stats|Lint|Info)` + the NEW `Session::integrity_check()`
//! read into a CLI-local `DoctorReport` (mapped onto `DiagnosticReport { kind: Info, .. }`). It does
//! NOT call `Session::doctor()`/`recover()` (the T3.3 `FeatureNotWired{"health"}` seam). **Non-zero
//! exit only on detected corruption:** a non-empty `integrity_check` → exit 2
//! (`ErrorCode::DatabaseError`); Lint/orphan findings are ADVISORY (reported, no exit flip); else exit
//! 0. The full Healthy/Drifted/Recoverable/Unsafe taxonomy + `--repair` land at T3.3.

use unblock_config::{CliOverrides, open_with_storage_with_cli};
use unblock_engine::{DiagnosticKind, DiagnosticReport, Session, SessionConfig};
use unblock_error::ErrorCode;

use crate::cli::DoctorArgs;
use crate::exit::CliError;
use crate::output::{self, DoctorReport, ToDiagnosticReport};

/// Run `unblock doctor` (doctor-lite).
///
/// # Errors
/// - [`CliError::Config`] if the workspace cannot be opened;
/// - [`CliError::Engine`] if the session cannot be opened or a diagnostics/integrity read fails;
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

    let stats = session.diagnostics(DiagnosticKind::Stats, None).await?;
    let lint = session.diagnostics(DiagnosticKind::Lint, None).await?;
    let info = session.diagnostics(DiagnosticKind::Info, None).await?;
    let integrity_problems = session.integrity_check().await?;

    let integrity_ok = integrity_problems.is_empty();
    let report = DoctorReport {
        integrity_ok,
        integrity_problems,
        stats: findings_of(&stats),
        lint: findings_of(&lint),
        info: findings_of(&info),
    };

    output::emit_report(&report.to_report(), fmt)?;

    // Non-zero exit ONLY on detected corruption (AF-1). The report was already emitted (it carries the
    // integrity problems); the exit code is derived from `ErrorCode::DatabaseError` (the SSOT for the
    // exit-2 db bucket — §2.3 unchanged, no new code).
    if integrity_ok {
        Ok(None)
    } else {
        Ok(Some(ErrorCode::DatabaseError.exit_code()))
    }
}

/// Flatten a `DiagnosticReport`'s findings into `(label, detail)` pairs (caller order preserved).
fn findings_of(report: &DiagnosticReport) -> Vec<(String, String)> {
    report
        .findings
        .iter()
        .map(|finding| (finding.label.clone(), finding.detail.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::findings_of;
    use unblock_engine::{DiagnosticFinding, DiagnosticKind, DiagnosticReport};

    #[test]
    fn findings_of_preserves_order_and_pairs() {
        let report = DiagnosticReport {
            kind: DiagnosticKind::Stats,
            findings: vec![
                DiagnosticFinding {
                    label: "open".to_string(),
                    detail: "3".to_string(),
                },
                DiagnosticFinding {
                    label: "closed".to_string(),
                    detail: "1".to_string(),
                },
            ],
        };
        let pairs = findings_of(&report);
        assert_eq!(
            pairs,
            vec![
                ("open".to_string(), "3".to_string()),
                ("closed".to_string(), "1".to_string()),
            ]
        );
    }

    #[test]
    fn database_error_exit_is_two() {
        use unblock_error::ErrorCode;
        assert_eq!(ErrorCode::DatabaseError.exit_code(), 2);
    }
}
