//! Workspace health before any libsql connection exists (FR-16 full, PRD §4 D58 clauses (6)–(9)).
//!
//! [`gather_preopen`] reads the file facts with `std::fs`. [`capture_sidecars`] reads the
//! anomalous `-shm` and `-wal` into memory before the gate's read-only open can touch them (tracker
//! comments 390 and 393). [`classify_preopen`] classifies the facts without a session, for the workspaces a
//! read-only open cannot serve. [`repair_preopen`] records the captured originals, runs the
//! exclusive actions inside `unblock_storage::exclusive_window` and records its evidence under
//! `.unblock/.recovery/`. [`HealthReport`] is also what `Session::doctor` returns, and the report
//! rendering here is shared with it.

use std::fs::{File, Metadata};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use unblock_config::ConfigPaths;
use unblock_error::CodedError;
use unblock_health::recovery::{
    self, Manifest, ManifestAction, ManifestFile, ManifestOriginal, Outcome, Phase, RunDir,
};
use unblock_health::{
    AdvisoryFacts, AnomalyClass, DbFileFact, ExportFacts, ExportScan, HealthError, HealthLevel,
    JOURNAL_SUFFIX, PlannedAction, PlannedRepair, PreOpenFacts, RepairPath, SHM_SUFFIX, WAL_SUFFIX,
    WorkspaceClassification, WorkspacePaths, sidecar,
};
use unblock_model::{DiagnosticFinding, DiagnosticKind, DiagnosticReport};
use unblock_storage::{ExclusiveOutcome, ExclusiveRequest, StorageError};

use crate::error::{EngineError, Result};

/// The 16-byte header every `SQLite` database file starts with.
const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";
/// The evidence root, a child of `.unblock/`.
const RECOVERY_DIR: &str = ".recovery";
/// The `VACUUM INTO` copy inside a run directory.
pub(crate) const SNAPSHOT_FILE: &str = "snapshot.db";
/// The run-directory subdirectories whose files the manifest hashes, in manifest order.
const EVIDENCE_DIRS: [&str; 3] = ["raw", "quarantine", "export"];
/// The version recorded in every manifest.
pub(crate) const BINARY_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A classified workspace: its level, the rendered report and the repairs it plans.
///
/// Engine-local, like `MigrateOutcome`. It is no spine §1.10 DTO and carries no `JsonSchema`, so
/// the wire contract does not move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthReport {
    /// The composite level (the `max` over the anomalies).
    pub level: HealthLevel,
    /// The findings, `DiagnosticKind::Info`, in the D58 clause (8) order.
    pub report: DiagnosticReport,
    /// One planned repair per anomaly, in class order.
    pub planned: Vec<PlannedRepair>,
}

/// What [`repair_preopen`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreOpenRepairOutcome {
    /// The run directory, or `None` when no exclusive action was planned.
    pub run_dir: Option<PathBuf>,
    /// One outcome per exclusive action, in plan order.
    pub actions: Vec<ActionOutcome>,
    /// Whether any action was refused.
    pub refused: bool,
}

/// One repair action and its result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionOutcome {
    /// The planned action.
    pub action: PlannedAction,
    /// What happened to it.
    pub outcome: ActionResult,
}

/// The result of one repair action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionResult {
    /// The action ran.
    Applied,
    /// The action did not run because exclusive access was not proven.
    Refused {
        /// The refusal code.
        reason: String,
    },
    /// The action ran and failed, or the phase failed before it ran.
    Failed {
        /// The error code or the missing effect.
        reason: String,
    },
}

/// One anomalous sidecar read into memory before any connection exists (tracker comment 390).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedSidecar {
    /// The file name, for example `unblock.db-shm`.
    pub name: String,
    /// The bytes as they were before the gate's read-only open.
    pub bytes: Vec<u8>,
    /// The lowercase hex SHA-256 of `bytes`.
    pub sha256: String,
}

/// Every anomalous sidecar captured by [`capture_sidecars`], in `-shm`, `-wal` order.
/// [`repair_preopen`] writes them into the run directory as the original evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SidecarCapture {
    /// The captured sidecars.
    pub files: Vec<CapturedSidecar>,
    /// Whether the gate's read-only open ran after the capture. The caller sets it. Only then does
    /// a captured `-shm` carry the "rebuilt by `SQLite`'s read-only open" note.
    pub gate_opened: bool,
}

/// The most bytes [`capture_sidecars`] keeps in memory for one sidecar (64 MiB, tracker comment
/// 393). A larger sidecar is a capture refusal.
pub const CAPTURE_CAP_BYTES: u64 = 64 * 1024 * 1024;

/// Gather the pre-open file facts of the workspace at `paths`.
///
/// Evaluates `unblock_storage::lost_database` first. It then refuses while this process holds a
/// libsql connection to the database file, because closing a `std::fs` descriptor on that file
/// drops every POSIX lock the process holds on it (`howtocorrupt.html` section 2.2). The rest is
/// `std::fs` only. A database header that cannot be read is recorded as
/// [`DbFileFact::Unreadable`], never returned as an error. The `.write.lock` probe never creates
/// the marker.
///
/// # Errors
/// - [`EngineError::PreOpenWithLiveConnection`] while a libsql connection to the database is open
///   in this process.
/// - [`StorageError::RecoveryIo`] when a sidecar or the marker cannot be inspected.
pub fn gather_preopen(paths: &ConfigPaths) -> Result<PreOpenFacts> {
    let db = &paths.db_path;
    let lost = unblock_storage::lost_database(db).map_err(|source| recovery_io(db, source))?;
    let live =
        unblock_storage::live_connections_to(db).map_err(|source| recovery_io(db, source))?;
    if live > 0 {
        return Err(EngineError::PreOpenWithLiveConnection { path: db.clone() });
    }
    let workspace = workspace_paths(paths);
    let wal = sidecar(db, WAL_SUFFIX);
    // `symlink_metadata`, like `file_exists`, so a dangling `-wal` symlink still counts as present.
    let wal_len = match std::fs::symlink_metadata(&wal) {
        Ok(meta) => Some(meta.len()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => return Err(recovery_io(&wal, source).into()),
    };
    Ok(PreOpenFacts {
        db: read_db_fact(db),
        lost,
        wal_len,
        shm_present: file_exists(&sidecar(db, SHM_SUFFIX))?,
        journal_present: file_exists(&sidecar(db, JOURNAL_SUFFIX))?,
        write_lock_held: unblock_storage::probe_write_lock(db)?,
        recovery_entries: recovery::scan_recovery_root(&workspace.recovery_dir),
    })
}

/// Classify pre-open facts without a session.
///
/// The cli renders this report alone when the facts forbid a read-only open. It holds no database
/// class, no schema rows and no dangling block. The manual path of a lost database is in its
/// `repair_planned` row.
#[must_use]
pub fn classify_preopen(facts: &PreOpenFacts) -> HealthReport {
    let export = ExportFacts {
        path: None,
        scan: ExportScan::Absent,
    };
    render(
        unblock_health::classify(facts, None, &export, &advisory_now()),
        Vec::new(),
    )
}

/// Read the anomalous `-shm` and `-wal` the facts show into memory, before any connection exists.
///
/// The `-shm` of `sidecar_mismatch` and the `-wal` of `truncated_wal` are captured, because the
/// gate's read-only open may rebuild them (tracker comment 390). A `-journal` is not captured: its
/// evidence is the window's `raw/` copy, taken before `SQLite` rolls it back (tracker comment 393).
/// The reads run on a blocking thread. A file is read only when it is a regular file (never through
/// a symlink) and the opened handle is that same file, and at most [`CAPTURE_CAP_BYTES`] bytes are
/// kept, in a buffer reserved up front.
///
/// # Errors
/// - [`EngineError::PreOpenWithLiveConnection`] while a libsql connection to the database is open
///   in this process.
/// - [`EngineError::SidecarCaptureRefused`] for a symlink, a non-regular file, a file that changes
///   between its check and its open, one above the cap, or a buffer that cannot be reserved.
/// - [`StorageError::RecoveryIo`] when a sidecar cannot be read.
pub async fn capture_sidecars(paths: &ConfigPaths, facts: &PreOpenFacts) -> Result<SidecarCapture> {
    let db = paths.db_path.clone();
    let live =
        unblock_storage::live_connections_to(&db).map_err(|source| recovery_io(&db, source))?;
    if live > 0 {
        return Err(EngineError::PreOpenWithLiveConnection { path: db });
    }
    let export = ExportFacts {
        path: None,
        scan: ExportScan::Absent,
    };
    let classification = unblock_health::classify(facts, None, &export, &advisory_now());
    // Class order puts the `-shm` first, then the `-wal`.
    let targets: Vec<PathBuf> = classification
        .anomalies
        .iter()
        .filter_map(|anomaly| match anomaly {
            AnomalyClass::SidecarMismatch => Some(SHM_SUFFIX),
            AnomalyClass::TruncatedWal { .. } => Some(WAL_SUFFIX),
            _ => None,
        })
        .map(|suffix| sidecar(&db, suffix))
        .collect();
    if targets.is_empty() {
        return Ok(SidecarCapture::default());
    }
    let read = tokio::task::spawn_blocking(move || {
        targets
            .iter()
            .map(|path| capture_one(path))
            .collect::<Result<Vec<_>>>()
    });
    let files = read
        .await
        .map_err(|join| recovery_io(&db, std::io::Error::other(join.to_string())))??;
    Ok(SidecarCapture {
        files,
        gate_opened: false,
    })
}

/// Run the planned exclusive actions inside `unblock_storage::exclusive_window`.
///
/// Only the actions whose path is [`RepairPath::PreopenExclusive`] run. With none, nothing is
/// written and `run_dir` is `None`. Otherwise the run directory exists before the window starts,
/// holding each `capture` file under `original/` (listed in the manifest's `originals`, apart from
/// the window's own copies) and the manifest. The capture is taken by value, so its bytes move to
/// the writer. Every run-directory copy is hashed on a blocking
/// thread, and the manifest is rewritten with each action's outcome. A refusal is an `Ok` outcome
/// with `refused: true`.
///
/// # Errors
/// - [`HealthError::RecoveryEvidenceWrite`] when the run directory, an original, a hash or the
///   manifest cannot be written.
/// - The window's own error (`StorageError::RecoveryIo` on its file I/O), after the manifest
///   records every action as `failed`.
pub async fn repair_preopen(
    paths: &ConfigPaths,
    planned: &[PlannedRepair],
    capture: SidecarCapture,
) -> Result<PreOpenRepairOutcome> {
    let exclusive: Vec<&PlannedRepair> = planned
        .iter()
        .filter(|repair| repair.path == RepairPath::PreopenExclusive)
        .collect();
    if exclusive.is_empty() {
        return Ok(PreOpenRepairOutcome {
            run_dir: None,
            actions: Vec::new(),
            refused: false,
        });
    }
    let actions: Vec<PlannedAction> = exclusive.iter().map(|r| r.action.clone()).collect();

    let run = create_run_dir(workspace_paths(paths).recovery_dir).await?;
    // Every exclusive action repairs a `Recoverable` sidecar class, because no `Unsafe` class
    // plans one (PRD §4 D58 clause (1), tracker comment 389).
    let mut manifest = Manifest::new(
        &run,
        Phase::Preopen,
        BINARY_VERSION,
        None,
        HealthLevel::Recoverable,
    );
    let SidecarCapture { files, gate_opened } = capture;
    for captured in files {
        manifest
            .originals
            .push(write_original(&run, captured, gate_opened).await?);
    }
    write_manifest(&run, &manifest).await?;

    let mut request = ExclusiveRequest::new(run.path().to_path_buf());
    for action in &actions {
        request = match action {
            PlannedAction::QuarantineSidecar => request.quarantine_shm(),
            PlannedAction::QuarantineJournal => request.quarantine_journal(),
            PlannedAction::CheckpointTruncate => request.checkpoint_truncate(),
            _ => request,
        };
    }
    let window = unblock_storage::exclusive_window(&paths.db_path, &request).await;

    let outcomes: Vec<ActionOutcome> = actions
        .into_iter()
        .map(|action| {
            let outcome = match &window {
                Ok(done) => window_result(&action, done, &paths.db_path),
                Err(err) => ActionResult::Failed {
                    reason: error_code(err),
                },
            };
            ActionOutcome { action, outcome }
        })
        .collect();
    manifest.actions = outcomes
        .iter()
        .map(|o| manifest_action(&o.action, &o.outcome))
        .collect();
    let recorded = finish_manifest(&run, &mut manifest).await;
    window?;
    recorded?;

    Ok(PreOpenRepairOutcome {
        run_dir: Some(run.path().to_path_buf()),
        refused: outcomes
            .iter()
            .any(|o| matches!(o.outcome, ActionResult::Refused { .. })),
        actions: outcomes,
    })
}

/// Render a pre-open repair outcome as findings.
///
/// `Session::recover` renders its actions the same way, so the cli can show both phases in one
/// document. Each action is a `repair` row and the run directory is the trailing `evidence` row.
#[must_use]
pub fn outcome_findings(outcome: &PreOpenRepairOutcome) -> Vec<DiagnosticFinding> {
    let mut findings: Vec<DiagnosticFinding> = outcome
        .actions
        .iter()
        .map(|o| action_finding(&o.action, &o.outcome))
        .collect();
    if let Some(run_dir) = &outcome.run_dir {
        findings.push(evidence_finding(run_dir));
    }
    findings
}

/// Build health's path bundle from config's resolved paths.
fn workspace_paths(paths: &ConfigPaths) -> WorkspacePaths {
    WorkspacePaths {
        db: paths.db_path.clone(),
        jsonl: Some(paths.jsonl_path.clone()),
        recovery_dir: recovery_root(&paths.unblock_dir),
    }
}

/// The evidence root `.unblock/.recovery/`.
pub(crate) fn recovery_root(unblock_dir: &Path) -> PathBuf {
    unblock_dir.join(RECOVERY_DIR)
}

/// The advisory inputs at the current instant.
pub(crate) fn advisory_now() -> AdvisoryFacts {
    AdvisoryFacts {
        now_utc: SystemTime::now(),
    }
}

/// Render a classification in the D58 clause (8) order: the `health` level row, the anomalies,
/// one `repair_planned` row per planned action, the advisories, then `tail` (the D46 schema rows
/// and the D45 dangling block for a session report).
pub(crate) fn render(
    classification: WorkspaceClassification,
    tail: Vec<DiagnosticFinding>,
) -> HealthReport {
    let WorkspaceClassification {
        level,
        anomalies,
        advisories,
        planned,
        ..
    } = classification;
    let mut findings =
        Vec::with_capacity(1 + anomalies.len() + planned.len() + advisories.len() + tail.len());
    findings.push(finding("health", level.as_str()));
    findings.extend(anomalies.iter().map(|a| finding(a.code(), a.to_string())));
    findings.extend(
        planned
            .iter()
            .map(|repair| finding("repair_planned", planned_detail(repair))),
    );
    findings.extend(advisories.iter().map(|a| finding(a.code(), a.to_string())));
    findings.extend(tail);
    HealthReport {
        level,
        report: DiagnosticReport {
            kind: DiagnosticKind::Info,
            findings,
        },
        planned,
    }
}

/// One diagnostic row.
pub(crate) fn finding(label: impl Into<String>, detail: impl Into<String>) -> DiagnosticFinding {
    DiagnosticFinding {
        label: label.into(),
        detail: detail.into(),
    }
}

/// The `repair` row of one action: `<code>[ <target>]: applied | refused (<reason>) | failed
/// (<reason>)`.
pub(crate) fn action_finding(action: &PlannedAction, result: &ActionResult) -> DiagnosticFinding {
    let outcome = match result {
        ActionResult::Applied if *action == PlannedAction::QuarantineJournal => {
            JOURNAL_ROLLED_BACK_DETAIL.to_string()
        }
        ActionResult::Applied => "applied".to_string(),
        ActionResult::Refused { reason } => format!("refused ({reason})"),
        ActionResult::Failed { reason } => format!("failed ({reason})"),
    };
    finding("repair", format!("{}: {outcome}", action_label(action)))
}

/// The `evidence` row naming a run directory.
pub(crate) fn evidence_finding(run_dir: &Path) -> DiagnosticFinding {
    finding("evidence", run_dir.display().to_string())
}

/// The manifest row of one action.
pub(crate) fn manifest_action(action: &PlannedAction, result: &ActionResult) -> ManifestAction {
    let (outcome, reason) = match result {
        ActionResult::Applied if *action == PlannedAction::QuarantineJournal => {
            (Outcome::Applied, Some(JOURNAL_ROLLED_BACK.to_string()))
        }
        ActionResult::Applied => (Outcome::Applied, None),
        ActionResult::Refused { reason } => (Outcome::Refused, Some(reason.clone())),
        ActionResult::Failed { reason } => (Outcome::Failed, Some(reason.clone())),
    };
    ManifestAction {
        action: action.code().to_string(),
        target: action.target(),
        outcome,
        reason,
    }
}

/// Hash every evidence file in the run directory and rewrite the manifest.
pub(crate) async fn finish_manifest(run: &RunDir, manifest: &mut Manifest) -> Result<()> {
    manifest.files = hash_evidence(run.path().to_path_buf()).await?;
    write_manifest(run, manifest).await
}

/// Create a run directory under `recovery_root` on a blocking thread.
pub(crate) async fn create_run_dir(recovery_root: PathBuf) -> Result<RunDir> {
    let root = recovery_root.clone();
    on_blocking_thread(&recovery_root, move || {
        RunDir::create(&root, SystemTime::now(), std::process::id())
    })
    .await
}

/// Write `manifest.json` into `run` on a blocking thread.
pub(crate) async fn write_manifest(run: &RunDir, manifest: &Manifest) -> Result<()> {
    let (owned_run, owned_manifest) = (run.clone(), manifest.clone());
    on_blocking_thread(run.path(), move || {
        owned_run.write_manifest(&owned_manifest)
    })
    .await
}

/// Copy the export at `src` into `run`'s `export/` on a blocking thread, refusing a source
/// outside `confine_root`.
pub(crate) async fn copy_export(run: &RunDir, src: &Path, confine_root: &Path) -> Result<PathBuf> {
    let owned_run = run.clone();
    let (owned_src, owned_root) = (src.to_path_buf(), confine_root.to_path_buf());
    on_blocking_thread(run.path(), move || {
        owned_run.copy_export(&owned_src, &owned_root)
    })
    .await
}

/// Scan the export at `path` for conflict markers on a blocking thread.
pub(crate) async fn scan_export(path: Option<PathBuf>) -> Result<ExportFacts> {
    let probed = path.clone().unwrap_or_default();
    let scan = tokio::task::spawn_blocking(move || unblock_health::scan_export(path.as_deref()));
    let facts = scan.await.map_err(|join| HealthError::Io {
        source: std::io::Error::other(join.to_string()),
        path: probed,
    })?;
    Ok(facts)
}

/// Run the evidence I/O `work` on a blocking thread. A task that cannot be joined is a
/// [`HealthError::RecoveryEvidenceWrite`] on `path`.
async fn on_blocking_thread<T: Send + 'static>(
    path: &Path,
    work: impl FnOnce() -> std::result::Result<T, HealthError> + Send + 'static,
) -> Result<T> {
    let joined = tokio::task::spawn_blocking(work).await.map_err(|join| {
        HealthError::RecoveryEvidenceWrite {
            source: std::io::Error::other(join.to_string()),
            path: path.to_path_buf(),
        }
    })?;
    Ok(joined?)
}

/// The manifest note of a captured lone `-shm`. The gate's read-only open rebuilt the live file
/// before the window renamed it into `quarantine/` (tracker comment 390).
const SHM_REBUILT_NOTE: &str = "rebuilt by SQLite's read-only open";

/// Write one captured sidecar into `run`'s `original/` on a blocking thread, and return its
/// manifest row with the capture's own SHA-256. A `-shm` carries the rebuilt note only when the
/// gate's read-only open ran (`gate_opened`).
async fn write_original(
    run: &RunDir,
    captured: CapturedSidecar,
    gate_opened: bool,
) -> Result<ManifestOriginal> {
    let CapturedSidecar {
        name,
        bytes,
        sha256,
    } = captured;
    let length = bytes.len() as u64;
    let rebuilt = gate_opened && name.ends_with(SHM_SUFFIX);
    let owned_run = run.clone();
    let written =
        on_blocking_thread(run.path(), move || owned_run.write_original(&name, &bytes)).await?;
    let relative = written.strip_prefix(run.path()).unwrap_or(&written);
    let path = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    Ok(ManifestOriginal {
        path,
        bytes: length,
        sha256,
        note: rebuilt.then(|| SHM_REBUILT_NOTE.to_string()),
    })
}

/// The blocking half of [`capture_sidecars`] for one file.
fn capture_one(path: &Path) -> Result<CapturedSidecar> {
    let cap = CAPTURE_CAP_BYTES;
    let before = std::fs::symlink_metadata(path).map_err(|source| recovery_io(path, source))?;
    if !before.file_type().is_file() {
        return Err(capture_refused(path, "a symlink or not a regular file"));
    }
    if before.len() > cap {
        return Err(over_the_cap(path, before.len(), cap));
    }
    let file = File::open(path).map_err(|source| recovery_io(path, source))?;
    let opened = file
        .metadata()
        .map_err(|source| recovery_io(path, source))?;
    if !opened.is_file() || !same_file(&before, &opened) {
        return Err(capture_refused(
            path,
            "the file changed between its check and its open",
        ));
    }
    let mut bytes = Vec::new();
    let reserve = usize::try_from(opened.len().min(cap.saturating_add(1))).unwrap_or(usize::MAX);
    if bytes.try_reserve_exact(reserve).is_err() {
        return Err(capture_refused(
            path,
            format!("cannot reserve {} bytes to hold it in memory", opened.len()),
        ));
    }
    file.take(cap.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|source| recovery_io(path, source))?;
    let len = bytes.len() as u64;
    if len > cap {
        return Err(over_the_cap(path, len, cap));
    }
    let sha256 = recovery::sha256_bytes(&bytes);
    Ok(CapturedSidecar {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        bytes,
        sha256,
    })
}

/// Whether two metadata records name the same file (device and inode on unix).
#[cfg(unix)]
fn same_file(a: &Metadata, b: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    a.dev() == b.dev() && a.ino() == b.ino()
}

/// Whether two metadata records name the same file. Windows has no stable file identity in
/// `std`, so the size and modification time stand in.
#[cfg(not(unix))]
fn same_file(a: &Metadata, b: &Metadata) -> bool {
    a.len() == b.len() && a.modified().ok() == b.modified().ok()
}

/// A capture refusal (`DATABASE_ERROR`, exit 2).
fn capture_refused(path: &Path, reason: impl Into<String>) -> EngineError {
    EngineError::SidecarCaptureRefused {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

/// The refusal of a sidecar above the export-copy cap.
fn over_the_cap(path: &Path, len: u64, cap: u64) -> EngineError {
    capture_refused(
        path,
        format!("{len} bytes exceeds the {cap}-byte cap of an evidence copy"),
    )
}

/// The stable code of an error, for a manifest `reason`.
pub(crate) fn error_code(err: &impl CodedError) -> String {
    err.code().as_str().to_string()
}

/// `<code>[ <target>]`.
fn action_label(action: &PlannedAction) -> String {
    match action.target() {
        Some(target) => format!("{} {target}", action.code()),
        None => action.code().to_string(),
    }
}

/// `<code>[ <target>] (<path>)`, plus the manual text for a manual action.
pub(crate) fn planned_detail(repair: &PlannedRepair) -> String {
    let mut detail = action_label(&repair.action);
    detail.push_str(" (");
    detail.push_str(repair.path.as_str());
    detail.push(')');
    if let PlannedAction::Manual { text } = &repair.action {
        detail.push_str(": ");
        detail.push_str(text);
    }
    detail
}

/// The manifest reason of an applied `quarantine_journal`: `SQLite` rolled the hot journal back and
/// unlinked it, and `raw/` holds the pre-rollback copy (PRD §4 D58 amendment).
const JOURNAL_ROLLED_BACK: &str = "rolled_back_by_sqlite; evidence raw/";

/// The report detail of an applied `quarantine_journal`.
const JOURNAL_ROLLED_BACK_DETAIL: &str =
    "applied (SQLite rolled the hot journal back; the pre-rollback copy is under raw/)";

/// The failure reason when the `-journal` is still present after the window's close.
const JOURNAL_STILL_PRESENT: &str = "journal_still_present";

/// What the window did for one action.
fn window_result(action: &PlannedAction, done: &ExclusiveOutcome, db: &Path) -> ActionResult {
    if let Some(refusal) = &done.refused {
        return ActionResult::Refused {
            reason: refusal.code().to_string(),
        };
    }
    let applied = match action {
        PlannedAction::QuarantineSidecar => {
            let shm = sidecar(db, SHM_SUFFIX);
            done.renamed.iter().any(|(from, _)| *from == shm)
        }
        // SQLite's rollback in step (c) is the repair and unlinks the journal at the close (PRD
        // §4 D58 amendment); the pre-rollback `raw/` copy is its evidence.
        PlannedAction::QuarantineJournal => {
            return if done.journal_rolled_back {
                ActionResult::Applied
            } else {
                ActionResult::Failed {
                    reason: JOURNAL_STILL_PRESENT.to_string(),
                }
            };
        }
        PlannedAction::CheckpointTruncate => done.checkpoint_truncated,
        _ => false,
    };
    if applied {
        ActionResult::Applied
    } else {
        ActionResult::Failed {
            reason: "not_performed".to_string(),
        }
    }
}

/// Hash `snapshot.db` and every file under `raw/`, `quarantine/` and `export/` on a blocking
/// thread. Each hash covers the copy inside the run directory.
async fn hash_evidence(run_dir: PathBuf) -> Result<Vec<ManifestFile>> {
    let dir = run_dir.clone();
    on_blocking_thread(&run_dir, move || evidence_files(&dir)).await
}

/// The blocking half of [`hash_evidence`].
fn evidence_files(run_dir: &Path) -> std::result::Result<Vec<ManifestFile>, HealthError> {
    let evidence_io = |source: std::io::Error, path: &Path| HealthError::RecoveryEvidenceWrite {
        source,
        path: path.to_path_buf(),
    };
    let mut files = Vec::new();
    let snapshot = run_dir.join(SNAPSHOT_FILE);
    if snapshot
        .try_exists()
        .map_err(|e| evidence_io(e, &snapshot))?
    {
        files.push(recovery::manifest_file(run_dir, &snapshot)?);
    }
    for sub in EVIDENCE_DIRS {
        let dir = run_dir.join(sub);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => return Err(evidence_io(source, &dir)),
        };
        let mut paths = Vec::new();
        for entry in entries {
            paths.push(entry.map_err(|e| evidence_io(e, &dir))?.path());
        }
        paths.sort();
        for path in paths {
            files.push(recovery::manifest_file(run_dir, &path)?);
        }
    }
    Ok(files)
}

/// Read the first 16 bytes of the database file. Never an error: a failed read is a fact.
fn read_db_fact(db: &Path) -> DbFileFact {
    let unreadable = |err: std::io::Error| DbFileFact::Unreadable {
        reason: err.to_string(),
    };
    let len = match std::fs::metadata(db) {
        Ok(meta) => meta.len(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return DbFileFact::Absent,
        Err(err) => return unreadable(err),
    };
    if len == 0 {
        return DbFileFact::Empty;
    }
    let mut header = Vec::with_capacity(SQLITE_MAGIC.len());
    let read = std::fs::File::open(db).and_then(|file| file.take(16).read_to_end(&mut header));
    match read {
        Err(err) => unreadable(err),
        Ok(_) if header.as_slice() == SQLITE_MAGIC => DbFileFact::SqliteHeader,
        Ok(_) => DbFileFact::NotSqlite { len },
    }
}

/// Whether a file exists, as a storage-side I/O error when the check itself fails.
fn file_exists(path: &Path) -> Result<bool> {
    // `symlink_metadata`, so a dangling symlink still counts as present and reaches the window,
    // which refuses it as `not_a_regular_file`.
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(recovery_io(path, source).into()),
    }
}

/// A file I/O failure on a pre-open path (`IO_ERROR`, exit 8).
fn recovery_io(path: &Path, source: std::io::Error) -> StorageError {
    StorageError::RecoveryIo {
        path: path.to_path_buf(),
        source,
    }
}
