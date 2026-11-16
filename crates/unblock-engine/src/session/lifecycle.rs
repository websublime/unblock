//! `impl Session` lifecycle — `open`, `shutdown`, `migrate`, and the FR-16 health pair `doctor`
//! (classify) and `recover` (online repair), both built on `unblock-health` (PRD §4 D58).
//!
//! `open` **consumes** a `WorkspaceContext` built by `unblock-config` (CF-D): config already did
//! `.unblock/` discovery, opened/migrated libsql, and built the `Arc<dyn Storage>` — the engine takes
//! `ctx.storage`/`ctx.workspace_dir`/`ctx.config`/`ctx.reloader`/`ctx.paths` and **does not**
//! construct storage or run migrations itself. It builds the single write `Semaphore(1)` (D14), the
//! live runtime snapshot (D57) and wires its own (never-set) shutdown flag (the cli installs the OS
//! handler later via [`Session::with_shutdown_flag`], OQ-4).
//!
//! D57 clause (2a): `open` and `shutdown` are EXEMPT from the fail-closed gate (a gated workspace
//! must still open — so `unblock mcp` and its repair action stay reachable — and always close);
//! `migrate`, `doctor` and `recover` apply it.

#[cfg(feature = "health")]
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use tokio::sync::Semaphore;
use unblock_config::WorkspaceContext;
#[cfg(feature = "health")]
use unblock_health::recovery::{Manifest, Phase, RunDir};
#[cfg(feature = "health")]
use unblock_health::{
    DbFacts, DbFileFact, HealthLevel, PlannedAction, PreOpenFacts, RepairPath,
    WorkspaceClassification, WorkspacePaths,
};
#[cfg(feature = "health")]
use unblock_model::DiagnosticKind;
use unblock_model::DiagnosticReport;
#[cfg(feature = "health")]
use unblock_storage::{DbHealthFacts, RepairAction, RepairOutcome, RepairRequest};

use crate::error::{EngineError, Result};
use crate::permit::{WRITE_PERMITS, acquire_write};
#[cfg(feature = "health")]
use crate::preopen::{self, ActionResult, HealthReport};
use crate::session::{Session, SessionConfig};

/// The outcome of an idempotent [`Session::migrate`] run (D27/AF-2, T3.1 — spine §4.1).
///
/// `from`/`to` are the on-disk `PRAGMA user_version` observed before/after the migrate call;
/// `applied` is `from != to` — `true` only when the call actually advanced the schema (a genuinely
/// fresh/never-migrated DB, or a future v1.1 forward step). On a workspace opened via the config
/// facade (which migrates on open) `migrate` is a no-op and `applied == false` — an honest
/// idempotent signal, not a phantom applied-list.
///
/// This is an **engine-local** return type (NOT a spine §1.10 model DTO; no `JsonSchema`) — the peer
/// of the engine-local `ImportOptions`, not of the model-owned `CloseOutcome`. The cli maps it onto a
/// `DiagnosticReport` for rendering (D27/AD-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrateOutcome {
    /// The on-disk schema version BEFORE this migrate call.
    pub from: i64,
    /// The on-disk schema version AFTER this migrate call.
    pub to: i64,
    /// Whether the migrate advanced the schema (`from != to`).
    pub applied: bool,
}

impl Session {
    /// Open a session over an already-built [`WorkspaceContext`] (CF-D, spine §4.1).
    ///
    /// Consumes the storage-bearing context (config built `Arc<dyn Storage>` and migrated the DB),
    /// builds the single write permit, and wires a fresh never-set shutdown flag. If
    /// `cfg.import_on_open` is `true` the import seam runs over `ctx.paths.jsonl_path` under the
    /// write permit — but in **v1** that seam delegates to the still-stub `unblock-sync` (T2.4), so
    /// `open(import_on_open=true)` returns the typed [`EngineError::FeatureNotWired`] (`"sync"`) and
    /// applies **no** DB write (never a faked import). The flag-on path is wired; only the body is
    /// the seam.
    ///
    /// `open` is EXEMPT from the D57 clause (2a) fail-closed gate: a context whose runtime snapshot
    /// holds a blocking DB config fault still opens, so the `config` repair action stays reachable.
    ///
    /// # Errors
    ///
    /// - [`EngineError::FeatureNotWired`] (`feature: "sync"`) if `cfg.import_on_open` is `true` (the
    ///   sync seam is unwired until T2.4) — applied **before** any mutation, so the DB is untouched.
    // async per the spine §4.1 signature; the import-on-open seam (T2.4) awaits the sync delegation.
    #[allow(clippy::unused_async)]
    pub async fn open(ctx: WorkspaceContext, cfg: SessionConfig) -> Result<Self> {
        let WorkspaceContext {
            storage,
            workspace_dir,
            config,
            // D57 clause (8): the reloader moves into the session — `config_set`/`config_unset`
            // re-resolve the runtime keys through it.
            reloader,
            paths,
            // The D39 discovery-tier tag is surfaced by the CLI at startup, not consumed by the engine.
            source: _,
            // D46 clause (10): the facade's PRE-migration stamp is read only by the cli `migrate`
            // command (which copies it out before this call consumes the context), never by the
            // engine — the same additive shape `source` takes, so no layering edge moves.
            schema_version_before_migrate: _,
        } = ctx;

        let session = Self {
            storage,
            write_permit: Arc::new(Semaphore::new(WRITE_PERMITS)),
            config: config.startup,
            runtime: Arc::new(RwLock::new(Arc::new(config.runtime))),
            reloader,
            workspace_dir,
            unblock_dir: paths.unblock_dir,
            db_path: paths.db_path,
            jsonl_path: paths.jsonl_path,
            knobs: cfg,
            shutdown: Arc::new(AtomicBool::new(false)),
        };

        // import_on_open is a wired flag whose v1 body is the sync seam (T2.4). We surface the typed
        // not-wired error WITHOUT touching the DB (the import has not run, so nothing was written).
        if session.knobs.import_on_open {
            return Err(EngineError::FeatureNotWired { feature: "sync" });
        }

        Ok(session)
    }

    /// Cooperatively shut the session down (FR-17 AC). EXEMPT from the D57 fail-closed gate: a
    /// session always closes cleanly.
    ///
    /// Flips the shutdown flag (so subsequent `acquire_write` calls fail fast with
    /// [`EngineError::ShutdownInProgress`]), then **drains the in-flight permit** — it acquires the
    /// single write permit, which only succeeds once any in-flight mutation has released it, so the
    /// returned `Ok(())` witnesses that no write is mid-transaction. Dropping the drained permit
    /// leaves the libsql connection idle for a clean close (the backend closes its connections on
    /// `Drop`). Idempotent: a second `shutdown()` is a no-op `Ok(())`.
    ///
    /// # Errors
    ///
    /// - [`EngineError::WritePermitPoisoned`] if the semaphore was already closed.
    pub async fn shutdown(&self) -> Result<()> {
        // Idempotent: flipping an already-set flag is harmless; the drain below still completes.
        self.shutdown.store(true, Ordering::SeqCst);

        // Drain: acquire the single permit so any in-flight writer has finished its tx (committed or
        // rolled back — cancel-safe, spine §4.2). The acquired permit is dropped immediately; we do
        // NOT close the semaphore (a second shutdown must still be able to drain).
        let _drained = Arc::clone(&self.write_permit)
            .acquire_owned()
            .await
            .map_err(|_closed| EngineError::WritePermitPoisoned)?;

        Ok(())
    }

    /// Ensure the schema is at the version this build expects, idempotently, and report the from→to
    /// delta (D27/AF-2, T3.1 — spine §4.1).
    ///
    /// Runs **under the single write permit** (D14 — migration is a write-path op): it reads
    /// `from = storage.schema_version()` under the held permit (a consistent snapshot no interleaved
    /// writer can advance), runs the idempotent `storage.migrate()` (a no-op on a current DB), re-reads
    /// `to`, and returns `MigrateOutcome { from, to, applied: from != to }`. A database stamped at a
    /// version NEWER than this build surfaces the transparent [`StorageError::SchemaMismatch`] (→ exit
    /// 2) — never a fake success. Because the config open facade migrates on open (FR-9 single open
    /// path), `applied` is normally `false` post-open. Backs the cli `migrate` command.
    ///
    /// **D46 (v1.0.1) — this BODY is unchanged and `from` keeps its published meaning (the stamp THIS
    /// call observed), but two of its outcomes move without a code change here.** `applied` becomes
    /// `true` on its own wherever THIS call precedes the ladder — a store the config open facade did
    /// not already migrate — and, because the frozen-baseline discipline makes even a FRESH database
    /// reach the current shape THROUGH the ladder, a never-migrated store reports `from: 0`,
    /// `to: CURRENT_SCHEMA_VERSION`, `applied: true`. So `applied: false` is a property of an
    /// ALREADY-OPENED workspace, never of a fresh one. **What this method does NOT become
    /// (D46 clause (10)):** on a facade-opened workspace `from == to` and `applied: false` still hold.
    /// The cli `migrate` command's honest `1` → `2` `applied: true` is sourced from
    /// `WorkspaceContext::schema_version_before_migrate`, NOT from `MigrateOutcome`; widening `from`
    /// to mean the pre-open stamp was REJECTED, since it would make this type report a version its
    /// own call never observed and would falsify the post-open idempotence cell.
    ///
    /// # Errors
    /// - [`EngineError::Config`] (`FaultyDbConfig`) while a blocking DB config fault exists (D57
    ///   clause (2a) — checked after the permit, before any storage call).
    /// - [`EngineError::ShutdownInProgress`] if a shutdown is in progress (the permit is refused up
    ///   front, before any read/migrate).
    /// - The transparent storage source (`SchemaMismatch`/`Migration`/backend read) on any failure.
    ///
    /// [`StorageError::SchemaMismatch`]: unblock_storage::StorageError::SchemaMismatch
    pub async fn migrate(&self) -> Result<MigrateOutcome> {
        // Write permit (D14), shutdown-aware. `Session::acquire` (write.rs) is private to the `write`
        // module, so — like `interchange.rs` — the lifecycle path calls the crate helper directly.
        let _guard = acquire_write(&self.write_permit, &self.shutdown).await?;
        // D57 clause (2a): the fail-closed gate, on the snapshot taken under the permit.
        self.gated_runtime()?;
        // Read `from` UNDER the permit so the read + migrate see one serialized writer window.
        let from = self.storage.schema_version().await?;
        self.storage.migrate().await?; // idempotent; a no-op on a current DB.
        let to = self.storage.schema_version().await?;
        Ok(MigrateOutcome {
            from,
            to,
            applied: from != to,
        })
    }

    /// Classify the workspace (FR-16 full, PRD §4 D58 clause (8)).
    ///
    /// Call it on a session opened `ReadOnly`, with `preopen` gathered by
    /// [`gather_preopen`](crate::preopen::gather_preopen) before that open. It is a pure read: no
    /// permit, no `.write.lock`, no migration. It makes ONE `Storage::health_facts()` call, whose
    /// rows are the one `integrity_check` of this classification, scans the export with
    /// `unblock_health::scan_export`, and classifies with the pure `unblock_health::classify`.
    ///
    /// The report order is NORMATIVE. The `health` level row comes first, then the anomalies in
    /// class order, one `repair_planned` row per planned action, the advisories, and the two D46
    /// rows (`schema_version` observed on disk, then `schema_expected` for this build). The D45
    /// dangling block is the trailing suffix. It comes from the same fn the
    /// `diagnostics {kind:"dangling"}` action calls, so the two cannot drift.
    ///
    /// # Errors
    /// - [`EngineError::Config`] (`FaultyDbConfig`) while a blocking DB config fault exists (D57
    ///   clause (2a)), before any storage call.
    /// - The transparent storage source of the `health_facts` or dangling read.
    #[cfg(feature = "health")]
    pub async fn doctor(&self, preopen: &PreOpenFacts) -> Result<HealthReport> {
        self.gated_runtime()?;
        let db = db_facts(self.storage.health_facts().await?);
        let export = preopen::scan_export(Some(self.jsonl_path.clone())).await?;
        let classification =
            unblock_health::classify(preopen, Some(&db), &export, &preopen::advisory_now());
        let mut tail = vec![
            preopen::finding("schema_version", db.schema_version.to_string()),
            preopen::finding("schema_expected", db.expected_schema_version.to_string()),
        ];
        tail.extend(crate::diagnostics::dangling_findings(self.storage.as_ref()).await?);
        Ok(preopen::render(classification, tail))
    }

    /// Health's path bundle for this session's workspace.
    #[cfg(feature = "health")]
    fn health_paths(&self) -> WorkspacePaths {
        WorkspacePaths {
            db: self.db_path.clone(),
            jsonl: Some(self.jsonl_path.clone()),
            recovery_dir: preopen::recovery_root(&self.unblock_dir),
        }
    }

    /// Run the online repairs (FR-16 full, PRD §4 D58 clause (6)).
    ///
    /// Call it on a session opened `NoMigrate`. The steps run in this order:
    /// 1. the D57 fail-closed gate;
    /// 2. the write permit and the `.write.lock`, held to the end with the configured timeout (a
    ///    timeout is the retryable `DATABASE_LOCKED`);
    /// 3. ONE `health_facts()` call and `scan_export`, classifying the online classes;
    /// 4. the Unsafe gate: with any `Unsafe` class, a report of those classes, their manual path
    ///    and a `recover` row, with no action row and no directory (tracker comment 389);
    /// 5. with no online action, a report with no action rows, and no directory;
    /// 6. otherwise one run directory under `.unblock/.recovery/` with its manifest, a copy of a
    ///    poisoned export at `export/issues.jsonl`, and ONE `Storage::repair` call (the snapshot,
    ///    then `migrate` when planned, then one action transaction);
    /// 7. the export, regenerated through the atomic sync exporter;
    /// 8. the manifest, rewritten with every outcome.
    ///
    /// It never runs an exclusive action; those belong to
    /// [`repair_preopen`](crate::preopen::repair_preopen). The report holds one `repair` row per
    /// action, an `export_not_imported` row after a regenerated export, and the trailing
    /// `evidence` row. It reports no final level.
    ///
    /// # Errors
    /// - [`EngineError::Config`] (`FaultyDbConfig`) while a blocking DB config fault exists.
    /// - [`EngineError::ShutdownInProgress`] when a shutdown is in progress.
    /// - The storage error of a failed `repair`, or the sync error of a failed regeneration, after
    ///   the manifest records `failed` with its code. A `migrate` whose ladder reached this build's
    ///   stamp before the failure is recorded `applied`.
    /// - The health error of an evidence write.
    #[cfg(feature = "health")]
    pub async fn recover(&self) -> Result<DiagnosticReport> {
        self.gated_runtime()?;
        let _guard = acquire_write(&self.write_permit, &self.shutdown).await?;
        // Declared after the permit, so it drops first (spine §4.2).
        let _lock = self.storage.acquire_write_lock().await?;

        let paths = self.health_paths();
        let db = db_facts(self.storage.health_facts().await?);
        let export = preopen::scan_export(paths.jsonl.clone()).await?;
        let classification = unblock_health::classify(
            &online_preopen(),
            Some(&db),
            &export,
            &preopen::advisory_now(),
        );
        if classification.level == HealthLevel::Unsafe {
            return Ok(unsafe_gate_report(&classification));
        }
        let actions: Vec<PlannedAction> = classification
            .planned
            .iter()
            .filter(|repair| repair.path == RepairPath::Online)
            .map(|repair| repair.action.clone())
            .collect();
        if actions.is_empty() {
            return Ok(DiagnosticReport {
                kind: DiagnosticKind::Info,
                findings: vec![preopen::finding("recover", "nothing to repair online")],
            });
        }

        let run = preopen::create_run_dir(paths.recovery_dir).await?;
        // NFR-13: one audit record per mutating phase, naming its run directory.
        let mut audit = classification.audit_record("recover");
        audit.path = run.path().display().to_string();
        audit.emit();
        let manifest = Manifest::new(
            &run,
            Phase::Online,
            preopen::BINARY_VERSION,
            Some(db.schema_version),
            classification.level,
        );
        preopen::write_manifest(&run, &manifest).await?;
        let mut online = OnlineRun {
            run,
            manifest,
            results: vec![None; actions.len()],
            actions,
        };

        let regenerate = online.position(&PlannedAction::RegenerateExport);
        let export_copy = match (&export.path, regenerate) {
            (Some(path), Some(_)) => {
                match preopen::copy_export(&online.run, path, &self.unblock_dir).await {
                    Ok(copy) => Some(copy),
                    Err(err) => return Err(online.fail(err).await),
                }
            }
            _ => None,
        };

        let request = RepairRequest {
            snapshot: online.run.path().join(preopen::SNAPSHOT_FILE),
            migrate: online.position(&PlannedAction::Migrate).is_some(),
            actions: online.actions.iter().filter_map(repair_action).collect(),
        };
        let outcome = match self.storage.repair(&request).await {
            Ok(outcome) => outcome,
            Err(err) => {
                if let Some(index) = online.position(&PlannedAction::Migrate)
                    && self.migrated_to_current().await
                {
                    online.results[index] = Some(ActionResult::Applied);
                }
                return Err(online.fail(err.into()).await);
            }
        };
        for (action, result) in online.actions.iter().zip(online.results.iter_mut()) {
            *result = repair_result(action, &outcome);
        }

        if let Some(index) = regenerate {
            match self.write_export(&self.jsonl_path).await {
                Ok(_) => online.results[index] = Some(ActionResult::Applied),
                Err(err) => return Err(online.fail(err).await),
            }
        }

        online.finish().await?;
        Ok(online.report(export_copy.as_deref()))
    }

    /// Whether the on-disk stamp now equals this build's, after a failed `repair` whose ladder
    /// may have committed before its action transaction failed (D58 clause (5)). A stamp that
    /// cannot be read counts as not migrated.
    #[cfg(feature = "health")]
    async fn migrated_to_current(&self) -> bool {
        match self.storage.schema_version().await {
            Ok(stamp) => stamp == unblock_storage::CURRENT_SCHEMA_VERSION,
            Err(err) => {
                tracing::warn!(error = %err, "recover could not re-read the schema stamp");
                false
            }
        }
    }

    /// Attempt workspace repair — FR-16.
    ///
    /// # Errors
    /// - [`EngineError::Config`] (`FaultyDbConfig`) while a blocking DB config fault exists.
    /// - [`EngineError::FeatureNotWired`] (`feature: "health"`) otherwise: this build has no
    ///   `health` feature.
    #[cfg(not(feature = "health"))]
    #[allow(clippy::unused_async)] // async in the spine §4.1 signature; the health body awaits.
    pub async fn recover(&self) -> Result<DiagnosticReport> {
        self.gated_runtime()?;
        Err(EngineError::FeatureNotWired { feature: "health" })
    }
}

/// One `recover()` phase in progress: its run directory, manifest and per-action results.
/// `None` marks an action that has not run yet.
#[cfg(feature = "health")]
struct OnlineRun {
    run: RunDir,
    manifest: Manifest,
    actions: Vec<PlannedAction>,
    results: Vec<Option<ActionResult>>,
}

#[cfg(feature = "health")]
impl OnlineRun {
    /// The index of `wanted` in the plan.
    fn position(&self, wanted: &PlannedAction) -> Option<usize> {
        self.actions.iter().position(|action| action == wanted)
    }

    /// Hash the evidence and rewrite the manifest with every result.
    async fn finish(&mut self) -> Result<()> {
        let not_run = ActionResult::Failed {
            reason: "not_run".to_string(),
        };
        self.manifest.actions = self
            .actions
            .iter()
            .zip(&self.results)
            .map(|(action, result)| {
                preopen::manifest_action(action, result.as_ref().unwrap_or(&not_run))
            })
            .collect();
        preopen::finish_manifest(&self.run, &mut self.manifest).await
    }

    /// Record every action that has not run as `failed` with `err`'s code, then hand `err` back.
    /// A failure to record is logged, because `err` is the error the caller must see.
    async fn fail(mut self, err: EngineError) -> EngineError {
        let reason = preopen::error_code(&err);
        for result in self.results.iter_mut().filter(|result| result.is_none()) {
            *result = Some(ActionResult::Failed {
                reason: reason.clone(),
            });
        }
        if let Err(evidence) = self.finish().await {
            tracing::warn!(
                run_dir = %self.run.path().display(),
                error = %evidence,
                "recover could not record its failure in the manifest"
            );
        }
        err
    }

    /// One `repair` row per action, `export_not_imported` after a regenerated export, then the
    /// `evidence` row.
    fn report(&self, export_copy: Option<&Path>) -> DiagnosticReport {
        let mut findings = Vec::with_capacity(self.actions.len() + 2);
        for (action, result) in self.actions.iter().zip(&self.results) {
            let Some(result) = result else { continue };
            findings.push(preopen::action_finding(action, result));
            if let (PlannedAction::RegenerateExport, ActionResult::Applied, Some(copy)) =
                (action, result, export_copy)
            {
                findings.push(preopen::finding(
                    "export_not_imported",
                    format!(
                        "rows present only in the replaced export (for example a teammate's side \
                         of a merge) were NOT imported; its copy is {}; to keep them, resolve the \
                         conflict in that copy, then import it through the MCP `sync` tool's \
                         import action",
                        copy.display()
                    ),
                ));
            }
        }
        findings.push(preopen::evidence_finding(self.run.path()));
        DiagnosticReport {
            kind: DiagnosticKind::Info,
            findings,
        }
    }
}

/// The report of a `recover()` stopped by the Unsafe gate (PRD §4 D58, tracker comment 389).
///
/// It holds each `Unsafe` anomaly, its manual `repair_planned` row and a `recover` row. No in-product
/// repair is safe while such a class is present, so it holds no action row.
#[cfg(feature = "health")]
fn unsafe_gate_report(classification: &WorkspaceClassification) -> DiagnosticReport {
    let unsafe_codes: Vec<&'static str> = classification
        .anomalies
        .iter()
        .filter(|anomaly| anomaly.severity() == HealthLevel::Unsafe)
        .map(unblock_health::AnomalyClass::code)
        .collect();
    let mut findings: Vec<_> = classification
        .anomalies
        .iter()
        .filter(|anomaly| anomaly.severity() == HealthLevel::Unsafe)
        .map(|anomaly| preopen::finding(anomaly.code(), anomaly.to_string()))
        .collect();
    findings.extend(
        classification
            .planned
            .iter()
            .filter(|repair| unsafe_codes.contains(&repair.anomaly))
            .map(|repair| preopen::finding("repair_planned", preopen::planned_detail(repair))),
    );
    findings.push(preopen::finding(
        "recover",
        format!(
            "nothing repaired because an unsafe class is present ({}); resolve it by its manual \
             path first",
            unsafe_codes.join(", ")
        ),
    ));
    DiagnosticReport {
        kind: DiagnosticKind::Info,
        findings,
    }
}

/// Map storage's health facts into health's own type, so no storage type crosses into health.
#[cfg(feature = "health")]
fn db_facts(facts: DbHealthFacts) -> DbFacts {
    DbFacts {
        integrity_rows: facts.integrity_rows,
        schema_version: facts.schema_version,
        expected_schema_version: unblock_storage::CURRENT_SCHEMA_VERSION,
        index_names: facts.index_names,
        child_counter_drift: facts
            .child_counter_drift
            .into_iter()
            .map(|drift| (drift.parent_id, drift.stored, drift.observed_max))
            .collect(),
        duplicate_metadata_keys: facts
            .duplicate_metadata_keys
            .into_iter()
            .map(|dup| (dup.key, dup.rows))
            .collect(),
    }
}

/// Pre-open facts with no pre-open class. `recover()` classifies only the online classes, and a
/// live session may not read the database file (D58 clause (6)).
#[cfg(feature = "health")]
fn online_preopen() -> PreOpenFacts {
    PreOpenFacts {
        db: DbFileFact::SqliteHeader,
        lost: false,
        wal_len: None,
        shm_present: false,
        journal_present: false,
        write_lock_held: false,
        recovery_entries: Vec::new(),
    }
}

/// The SQL action of a planned online action, if it has one.
#[cfg(feature = "health")]
fn repair_action(action: &PlannedAction) -> Option<RepairAction> {
    match action {
        PlannedAction::Reindex { index } => Some(RepairAction::Reindex {
            index: index.clone(),
        }),
        PlannedAction::RaiseChildCounter { parent_id, to } => {
            Some(RepairAction::RaiseChildCounter {
                parent_id: parent_id.clone(),
                to: *to,
            })
        }
        PlannedAction::DedupeMetadataKey { key } => {
            Some(RepairAction::DedupeMetadataKey { key: key.clone() })
        }
        _ => None,
    }
}

/// What a successful `repair` call did for one action. `None` for an action `repair` does not
/// run (`regenerate_export`).
#[cfg(feature = "health")]
fn repair_result(action: &PlannedAction, outcome: &RepairOutcome) -> Option<ActionResult> {
    let applied = match action {
        PlannedAction::Migrate => outcome.migrated.is_some(),
        _ => outcome.applied.contains(&repair_action(action)?),
    };
    Some(if applied {
        ActionResult::Applied
    } else {
        ActionResult::Failed {
            reason: "not_applied".to_string(),
        }
    })
}
