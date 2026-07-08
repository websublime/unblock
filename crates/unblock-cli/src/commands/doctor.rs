//! `unblock doctor` (D27/AF-1 — doctor-LITE) — a read-only health report composed from the BUILD-now
//! diagnostics reads + the integrity probe.
//!
//! Opens the `Session` and composes `diagnostics(Stats|Lint|Info)` + the NEW `Session::integrity_check()`
//! read into a CLI-local `DoctorReport` (mapped onto `DiagnosticReport { kind: Info, .. }`). It does
//! NOT call `Session::doctor()`/`recover()` at T3.1 (the `FeatureNotWired{"health"}` seam); at **T3.3
//! (HEALTH-LITE, D29/F4)** this command is rewired to route through the now-wired `Session::doctor()`
//! (adding file-state anomalies), preserving this exit rule. **Non-zero exit only on detected
//! corruption:** a non-empty `integrity_check` → exit 2 (`ErrorCode::DatabaseError`); Lint/orphan
//! findings are ADVISORY (reported, no exit flip); else exit 0. The full
//! Healthy/Drifted/Recoverable/Unsafe taxonomy + `--repair` land at **v1.1**.

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

    // Assemble the report + derive the exit through the PURE decision helpers (below). Keeping the
    // decision out of the async fetch is what makes the AF-1 flip unit-testable WITHOUT a live
    // Session (the fetch above is the only impure part) — the corrupt-db integration test drives the
    // same helpers end-to-end.
    let report = build_doctor_report(&integrity_problems, &stats, &lint, &info);
    let exit = doctor_exit(&integrity_problems);

    output::emit_report(&report.to_report(), fmt)?;

    // The report was already emitted (it carries the integrity problems); `doctor_exit` derives the
    // non-zero exit ONLY on detected corruption (AF-1) — Lint/orphan findings stay advisory.
    Ok(exit)
}

/// Assemble the doctor-lite [`DoctorReport`] from the integrity probe + the three diagnostics reads
/// (D27/AF-1). PURE (no I/O): given the fetched inputs it deterministically produces the report — so
/// the "non-empty integrity is surfaced in the emitted report" invariant is unit-testable without a
/// live `Session`. `integrity_ok` is derived here (`integrity.is_empty()`), the single source for both
/// the report header and [`doctor_exit`].
fn build_doctor_report(
    integrity: &[String],
    stats: &DiagnosticReport,
    lint: &DiagnosticReport,
    info: &DiagnosticReport,
) -> DoctorReport {
    DoctorReport {
        integrity_ok: integrity.is_empty(),
        integrity_problems: integrity.to_vec(),
        stats: findings_of(stats),
        lint: findings_of(lint),
        info: findings_of(info),
    }
}

/// Derive the doctor exit code from the integrity probe (D27/AF-1). PURE: a NON-EMPTY `integrity`
/// (detected corruption) flips to `Some(ErrorCode::DatabaseError.exit_code())` (exit 2, the db
/// bucket — §2.3 SSOT, no new code); an empty probe is `None` (exit 0). Lint/orphan findings are
/// ADVISORY — they NEVER reach this function and NEVER flip the exit.
fn doctor_exit(integrity: &[String]) -> Option<u8> {
    if integrity.is_empty() {
        None
    } else {
        Some(ErrorCode::DatabaseError.exit_code())
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
    use super::{build_doctor_report, doctor_exit, findings_of};
    use unblock_engine::{DiagnosticFinding, DiagnosticKind, DiagnosticReport};

    /// An empty `DiagnosticReport` of `kind` (a healthy fetch's shape when a section has no findings).
    fn empty_report(kind: DiagnosticKind) -> DiagnosticReport {
        DiagnosticReport {
            kind,
            findings: Vec::new(),
        }
    }

    /// A `DiagnosticReport` of `kind` carrying a single `(label, detail)` finding.
    fn report_with(kind: DiagnosticKind, label: &str, detail: &str) -> DiagnosticReport {
        DiagnosticReport {
            kind,
            findings: vec![DiagnosticFinding {
                label: label.to_string(),
                detail: detail.to_string(),
            }],
        }
    }

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

    /// AF-1 flip (non-vacuous): a NON-EMPTY integrity probe → `Some(2)` (the db bucket). This
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

    /// AF-1: a clean (empty) integrity probe → `None` (exit 0). The healthy leg of the same branch.
    #[test]
    fn doctor_exit_empty_integrity_is_none() {
        assert_eq!(
            doctor_exit(&[]),
            None,
            "a clean integrity_check does not flip the exit"
        );
    }

    /// AF-1: Lint (advisory) findings alone NEVER flip the exit — only integrity does. `build_*`
    /// folds the Lint section into the report, but `doctor_exit` sees ONLY the integrity slice, so an
    /// empty integrity with a non-empty Lint report is still exit 0.
    #[test]
    fn advisory_lint_findings_alone_do_not_flip_exit() {
        let lint = report_with(DiagnosticKind::Lint, "stale", "2 issues untouched > 30d");
        let report = build_doctor_report(
            &[],
            &empty_report(DiagnosticKind::Stats),
            &lint,
            &empty_report(DiagnosticKind::Info),
        );
        // The advisory Lint finding IS folded into the report...
        assert!(report.integrity_ok, "empty integrity → integrity_ok");
        assert_eq!(
            report.lint,
            vec![("stale".to_string(), "2 issues untouched > 30d".to_string())],
            "advisory Lint findings are still reported"
        );
        // ...but it does NOT flip the exit (only integrity does).
        assert_eq!(
            doctor_exit(&[]),
            None,
            "advisory Lint findings never flip the exit"
        );
    }

    /// `build_doctor_report` surfaces the integrity findings in the assembled report AND sets
    /// `integrity_ok = false` when the probe is non-empty (the report the corruption case emits).
    #[test]
    fn build_report_includes_integrity_findings_when_non_empty() {
        let problems = vec![
            "*** in database main ***".to_string(),
            "page 5 is never used".to_string(),
        ];
        let report = build_doctor_report(
            &problems,
            &report_with(DiagnosticKind::Stats, "open", "0"),
            &empty_report(DiagnosticKind::Lint),
            &report_with(DiagnosticKind::Info, "actor", "alice"),
        );
        assert!(
            !report.integrity_ok,
            "a non-empty integrity probe sets integrity_ok = false"
        );
        assert_eq!(
            report.integrity_problems, problems,
            "the integrity findings are carried into the emitted report verbatim"
        );
        // The advisory sections are folded in alongside (proving composition, AF-1).
        assert_eq!(report.stats, vec![("open".to_string(), "0".to_string())]);
        assert_eq!(
            report.info,
            vec![("actor".to_string(), "alice".to_string())]
        );
    }
}
