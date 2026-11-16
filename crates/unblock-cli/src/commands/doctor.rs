//! `unblock doctor` and `unblock doctor --repair` — the phased FR-16 health command (PRD §4 D58
//! clause (8), spine §5b).
//!
//! The command opens no libsql connection until its pre-open work is done, because a descriptor on
//! the database file must never coexist with one (sqlite.org `howtocorrupt.html` section 2.2).
//!
//! 1. Phase (i) resolves the workspace without opening the database and gathers the pre-open file
//!    facts. When those facts forbid a read-only open, plain `doctor` renders the session-less
//!    pre-open report alone. Under `--repair`, the bytes of every anomalous sidecar are captured in
//!    memory here, while no connection is open, because the read-only open of phase (i-b) may
//!    rebuild a lone `-shm` and create an empty `-wal` and `-shm` (ruling 390).
//! 2. Phase (i-b), under `--repair` only, is the Unsafe gate. It classifies exactly as plain `doctor`
//!    does, and any `Unsafe` class renders that report, exits 2, repairs nothing and creates no
//!    `.recovery/` entry.
//! 3. Phase (ii), under `--repair` only, runs the pre-open exclusive actions planned from the
//!    phase (i) facts in one window, and writes the captured originals as evidence.
//! 4. Phase (iii) opens the database. Plain `doctor` opens it read-only and classifies once.
//!    `--repair` opens it without migrating and runs `Session::recover()`.
//! 5. Phase (iv), under `--repair` only, drops every connection, gathers fresh facts, and classifies
//!    again read-only for the final level.
//!
//! The exit comes from `HealthReport.level`: `Healthy` and `Drifted` exit 0, `Recoverable` and
//! `Unsafe` exit 2 through `ErrorCode::DatabaseError`. A refused pre-open action also exits 2. Plain
//! `doctor` changes no database content, takes no `.write.lock` and creates no `.recovery/` entry.

use unblock_config::{
    CliOverrides, ConfigError, ConfigPaths, OpenedContext, OutputFormat, StorageOpenMode,
    WorkspaceContext, open_with_cli,
};
use unblock_engine::preopen::{
    SidecarCapture, capture_sidecars, classify_preopen, gather_preopen, outcome_findings,
    repair_preopen,
};
use unblock_engine::{
    DbFileFact, DiagnosticKind, DiagnosticReport, EngineError, HealthLevel, HealthReport,
    PlannedRepair, PreOpenFacts, RepairPath, Session,
};
use unblock_error::ErrorCode;

use crate::cli::DoctorArgs;
use crate::dispatch::session_config;
use crate::exit::CliError;
use crate::output;

/// Run `unblock doctor`, or `unblock doctor --repair` when `args.repair` is set.
///
/// # Errors
/// - [`CliError::Config`] if the workspace cannot be resolved or opened, including a read-only open
///   that meets a hot journal while the re-gathered facts show no `-journal` (`DATABASE_ERROR`);
/// - [`CliError::Engine`] if gathering the pre-open facts, capturing a sidecar (an over-cap or
///   non-regular sidecar is refused with `DATABASE_ERROR`), the pre-open window's evidence I/O,
///   `Session::doctor()` or `Session::recover()` fails;
/// - [`CliError::Render`]/[`CliError::Io`] if rendering or writing the report fails.
pub async fn run(args: &DoctorArgs, overrides: &CliOverrides) -> Result<Option<u8>, CliError> {
    // Phase (i): resolve only. The database is untouched until the pre-open facts are in.
    let (paths, resolved_fmt) = resolve_only(overrides).await?;
    let facts = gather_preopen(&paths)?;

    if !args.repair {
        let (health, fmt, _) = classify(overrides, &paths, &facts, resolved_fmt).await?;
        output::emit_report(&health.report, fmt)?;
        return Ok(exit_for_level(health.level));
    }

    // Capture before touch: the originals are read while no connection is open.
    let mut capture = capture_sidecars(&paths, &facts).await?;
    // Phase (i-b): plain `doctor`'s classification. `classify` drops its session and context
    // before returning, and says whether it opened the database.
    let (gate, fmt, opened) = classify(overrides, &paths, &facts, resolved_fmt).await?;
    capture.gate_opened = opened;
    if unsafe_gate_stops_repair(gate.level) {
        drop(capture);
        output::emit_report(&gate.report, fmt)?;
        return Ok(exit_for_level(gate.level));
    }
    repair(overrides, &paths, &facts, capture, resolved_fmt).await
}

/// Run phases (ii) to (iv) of `--repair` and render them as ONE document.
///
/// The exclusive actions are planned from `facts`, the phase (i) facts, because the gate's read-only
/// open may have rebuilt a lone `-shm` and added an empty `-wal` beside it.
async fn repair(
    overrides: &CliOverrides,
    paths: &ConfigPaths,
    facts: &PreOpenFacts,
    capture: SidecarCapture,
    resolved_fmt: OutputFormat,
) -> Result<Option<u8>, CliError> {
    let mut findings = Vec::new();
    let mut refused = false;

    // Phase (ii): the pre-open exclusive actions, when any are planned. A database file without the
    // `SQLite` header stops before the window.
    if !stops_before_window(facts) {
        let exclusive = preopen_exclusive(classify_preopen(facts).planned);
        if !exclusive.is_empty() {
            let outcome = repair_preopen(paths, &exclusive, capture).await?;
            refused |= outcome.refused;
            findings.extend(outcome_findings(&outcome));
        }
    }

    // The short-circuit reads facts gathered after the gate and the window, so a journal SQLite
    // rolled back there no longer blocks the open, while one that survived still does.
    let facts = gather_preopen(paths)?;
    if !read_only_open_allowed(&facts) {
        let health = classify_preopen(&facts);
        return emit_repair_document(
            findings,
            &health.report,
            resolved_fmt,
            refused,
            health.level,
        );
    }

    // Phase (iii): the online repairs on a read-write, non-migrating open. The session owns the only
    // context, so dropping it closes every connection before phase (iv).
    let ctx = open_storage(overrides, StorageOpenMode::NoMigrate).await?;
    let session = Session::open(ctx, session_config()).await?;
    let recovered = session.recover().await?;
    drop(session);
    findings.extend(recovered.findings);

    // Phase (iv): fresh facts and the final read-only classification. `gather_preopen` refuses while
    // this process still holds any connection to the database.
    let facts = gather_preopen(paths)?;
    let (health, fmt, _) = classify(overrides, paths, &facts, resolved_fmt).await?;
    emit_repair_document(findings, &health.report, fmt, refused, health.level)
}

/// Render the repair findings followed by the final report as one document, and pick the exit.
fn emit_repair_document(
    mut findings: Vec<unblock_engine::DiagnosticFinding>,
    final_report: &DiagnosticReport,
    fmt: OutputFormat,
    refused: bool,
    level: HealthLevel,
) -> Result<Option<u8>, CliError> {
    findings.extend(final_report.findings.iter().cloned());
    let document = DiagnosticReport {
        kind: DiagnosticKind::Info,
        findings,
    };
    output::emit_report(&document, fmt)?;
    Ok(repair_exit(refused, level))
}

/// Classify the workspace for plain `doctor`, for the `--repair` gate and for the final `--repair`
/// pass.
///
/// Facts that forbid a read-only open yield the session-less pre-open report. Otherwise the database
/// opens read-only and `Session::doctor()` classifies it with one `integrity_check`. The returned
/// format is the one the opened context resolved, or `resolved_fmt` when no database was opened.
/// The flag says whether the read-only open ran.
async fn classify(
    overrides: &CliOverrides,
    paths: &ConfigPaths,
    facts: &PreOpenFacts,
    resolved_fmt: OutputFormat,
) -> Result<(HealthReport, OutputFormat, bool), CliError> {
    if !read_only_open_allowed(facts) {
        return Ok((classify_preopen(facts), resolved_fmt, false));
    }
    let ctx = match open_storage(overrides, StorageOpenMode::ReadOnly).await {
        Ok(ctx) => ctx,
        Err(CliError::Config { source }) if source.is_read_only_rollback() => {
            // A journal appeared after the pre-open read. Read the facts again before deciding.
            let refreshed = gather_preopen(paths)?;
            let health = after_read_only_rollback(source, &refreshed)?;
            return Ok((health, resolved_fmt, false));
        }
        Err(err) => return Err(err),
    };
    let fmt = ctx.config.runtime.output_format;
    let session = Session::open(ctx, session_config()).await?;
    let health = session.doctor(facts).await?;
    drop(session);
    Ok((health, fmt, true))
}

/// Open the workspace resolve-only (`no_db`, phase (i)) and return its paths and output format.
async fn resolve_only(overrides: &CliOverrides) -> Result<(ConfigPaths, OutputFormat), CliError> {
    match open_with_cli(&overrides.clone().with_no_db(true)).await? {
        OpenedContext::Resolved(ctx) => Ok((ctx.paths, ctx.config.runtime.output_format)),
        // `no_db` always resolves only; a storage context here is dropped (closing its
        // connections) before any pre-open read.
        OpenedContext::Storage(ctx) => Ok((ctx.paths.clone(), ctx.config.runtime.output_format)),
    }
}

/// A storage-bearing open in `mode` (never `no_db`).
async fn open_storage(
    overrides: &CliOverrides,
    mode: StorageOpenMode,
) -> Result<WorkspaceContext, CliError> {
    let cli = overrides.clone().with_no_db(false).with_storage_open(mode);
    match open_with_cli(&cli).await? {
        OpenedContext::Storage(ctx) => Ok(ctx),
        OpenedContext::Resolved(_) => Err(EngineError::WorkspaceNotOpen.into()),
    }
}

/// Whether the Unsafe gate (phase (i-b)) stops `--repair` before any repair. It does at
/// `Unsafe`, whose classes have no in-product repair, so the run repairs nothing and creates no
/// `.recovery/` entry.
fn unsafe_gate_stops_repair(level: HealthLevel) -> bool {
    matches!(level, HealthLevel::Unsafe)
}

/// Whether `--repair` stops before the pre-open window, which it does when the database file is
/// absent, empty, unreadable or not `SQLite`. The window never runs without a readable `SQLite`
/// database file.
fn stops_before_window(facts: &PreOpenFacts) -> bool {
    !matches!(facts.db, DbFileFact::SqliteHeader)
}

/// Whether the pre-open facts allow a read-only open. They do when the file carries the `SQLite`
/// header and no `-journal` sits beside it, because a read-only connection meeting a hot journal fails every read
/// with `SQLITE_READONLY_ROLLBACK`.
fn read_only_open_allowed(facts: &PreOpenFacts) -> bool {
    matches!(facts.db, DbFileFact::SqliteHeader) && !facts.journal_present
}

/// The planned actions that run in the pre-open exclusive window.
fn preopen_exclusive(planned: Vec<PlannedRepair>) -> Vec<PlannedRepair> {
    planned
        .into_iter()
        .filter(|repair| repair.path == RepairPath::PreopenExclusive)
        .collect()
}

/// A read-only open failed with `SQLITE_READONLY_ROLLBACK`. Facts gathered afterwards that show a
/// `-journal` yield the session-less pre-open report; otherwise the open error stands, because no
/// level was read.
fn after_read_only_rollback(
    err: ConfigError,
    refreshed: &PreOpenFacts,
) -> Result<HealthReport, ConfigError> {
    if refreshed.journal_present {
        Ok(classify_preopen(refreshed))
    } else {
        Err(err)
    }
}

/// The exit for a classified level. `Healthy`/`Drifted` exit 0, `Recoverable`/`Unsafe` exit 2
/// through `ErrorCode::DatabaseError`.
fn exit_for_level(level: HealthLevel) -> Option<u8> {
    match level {
        HealthLevel::Healthy | HealthLevel::Drifted => None,
        HealthLevel::Recoverable | HealthLevel::Unsafe => {
            Some(ErrorCode::DatabaseError.exit_code())
        }
    }
}

/// The `--repair` exit. Any refused pre-open action exits 2, else the final level decides.
fn repair_exit(refused: bool, level: HealthLevel) -> Option<u8> {
    if refused {
        Some(ErrorCode::DatabaseError.exit_code())
    } else {
        exit_for_level(level)
    }
}

#[cfg(test)]
mod tests {
    use unblock_config::ConfigError;
    use unblock_engine::{DbFileFact, HealthLevel, PreOpenFacts};
    use unblock_error::{CodedError as _, ErrorCode};
    use unblock_storage::StorageError;

    use super::{
        after_read_only_rollback, exit_for_level, read_only_open_allowed, repair_exit,
        stops_before_window, unsafe_gate_stops_repair,
    };

    fn facts(db: DbFileFact, journal_present: bool) -> PreOpenFacts {
        PreOpenFacts {
            db,
            lost: false,
            wal_len: None,
            shm_present: false,
            journal_present,
            write_lock_held: false,
            recovery_entries: Vec::new(),
        }
    }

    fn read_only_rollback() -> ConfigError {
        ConfigError::DbOpenFailed {
            source: StorageError::ReadOnlyRollback,
        }
    }

    #[test]
    fn database_error_exit_is_two() {
        assert_eq!(ErrorCode::DatabaseError.exit_code(), 2);
    }

    #[test]
    fn exit_for_level_is_0_for_healthy_and_drifted() {
        assert_eq!(exit_for_level(HealthLevel::Healthy), None);
        assert_eq!(exit_for_level(HealthLevel::Drifted), None);
    }

    #[test]
    fn exit_for_level_is_2_for_recoverable_and_unsafe() {
        assert_eq!(exit_for_level(HealthLevel::Recoverable), Some(2));
        assert_eq!(exit_for_level(HealthLevel::Unsafe), Some(2));
    }

    #[test]
    fn repair_exit_is_2_on_any_refusal_even_when_the_final_level_is_healthy() {
        assert_eq!(repair_exit(true, HealthLevel::Healthy), Some(2));
        assert_eq!(repair_exit(false, HealthLevel::Healthy), None);
        assert_eq!(repair_exit(false, HealthLevel::Drifted), None);
        assert_eq!(repair_exit(false, HealthLevel::Unsafe), Some(2));
    }

    #[test]
    fn only_a_sqlite_header_without_a_journal_allows_a_read_only_open() {
        assert!(read_only_open_allowed(&facts(
            DbFileFact::SqliteHeader,
            false
        )));
        assert!(!read_only_open_allowed(&facts(
            DbFileFact::SqliteHeader,
            true
        )));
        for db in [
            DbFileFact::Absent,
            DbFileFact::Empty,
            DbFileFact::Unreadable {
                reason: "denied".to_string(),
            },
            DbFileFact::NotSqlite { len: 5 },
        ] {
            assert!(!read_only_open_allowed(&facts(db.clone(), false)), "{db:?}");
        }
    }

    /// Ruling 389: only `Unsafe` stops `--repair` at the gate. Every other level reaches the repair
    /// phases.
    #[test]
    fn only_unsafe_stops_repair_at_the_gate() {
        assert!(unsafe_gate_stops_repair(HealthLevel::Unsafe));
        for level in [
            HealthLevel::Healthy,
            HealthLevel::Drifted,
            HealthLevel::Recoverable,
        ] {
            assert!(!unsafe_gate_stops_repair(level), "{level:?}");
        }
    }

    /// A lost, uncreated, unreadable or non-SQLite database stops `--repair` before the window. A
    /// journal beside a `SQLite` file reaches it.
    #[test]
    fn only_a_sqlite_header_reaches_the_window() {
        for db in [
            DbFileFact::Absent,
            DbFileFact::Empty,
            DbFileFact::Unreadable {
                reason: "denied".to_string(),
            },
            DbFileFact::NotSqlite { len: 5 },
        ] {
            assert!(stops_before_window(&facts(db.clone(), false)), "{db:?}");
        }
        assert!(!stops_before_window(&facts(DbFileFact::SqliteHeader, true)));
        assert!(!stops_before_window(&facts(
            DbFileFact::SqliteHeader,
            false
        )));
    }

    /// Criterion (h): a read-only open that failed `SQLITE_READONLY_ROLLBACK` while the re-gathered
    /// facts show no `-journal` keeps the open error, `DATABASE_ERROR`, exit 2.
    #[test]
    fn read_only_rollback_without_a_journal_keeps_the_database_error() {
        let err = after_read_only_rollback(
            read_only_rollback(),
            &facts(DbFileFact::SqliteHeader, false),
        )
        .expect_err("no journal: the open error stands");
        assert!(err.is_read_only_rollback());
        assert_eq!(err.code(), ErrorCode::DatabaseError);
        assert_eq!(err.code().exit_code(), 2);
    }

    /// Criterion (g): with a `-journal` now present, the fallback is the session-less pre-open report
    /// (`journal_sidecar_present`, `Recoverable`).
    #[test]
    fn read_only_rollback_with_a_journal_renders_the_session_less_report() {
        let health =
            after_read_only_rollback(read_only_rollback(), &facts(DbFileFact::SqliteHeader, true))
                .expect("a journal: the session-less report");
        assert_eq!(health.level, HealthLevel::Recoverable);
        assert_eq!(exit_for_level(health.level), Some(2));
        assert!(
            health
                .report
                .findings
                .iter()
                .any(|f| f.label == "journal_sidecar_present"),
            "{:?}",
            health.report
        );
    }
}
