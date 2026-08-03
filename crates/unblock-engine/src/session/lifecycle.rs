//! `impl Session` lifecycle — `open` / `shutdown` (BUILD-now) and `doctor` / `recover` (signatures
//! land now; bodies seamed to `unblock-health`, T3.3).
//!
//! `open` **consumes** a `WorkspaceContext` built by `unblock-config` (CF-D): config already did
//! `.unblock/` discovery, opened/migrated libsql, and built the `Arc<dyn Storage>` — the engine takes
//! `ctx.storage`/`ctx.workspace_dir`/`ctx.actor`/`ctx.config`/`ctx.paths` and **does not** construct
//! storage or run migrations itself. It builds the single write `Semaphore(1)` (D14) and wires its
//! own (never-set) shutdown flag (the cli installs the OS handler later via
//! [`Session::with_shutdown_flag`], OQ-4).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Semaphore;
use unblock_config::WorkspaceContext;
use unblock_model::DiagnosticReport;

use crate::error::{EngineError, Result};
use crate::permit::{WRITE_PERMITS, acquire_write};
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
            actor,
            config,
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
            config,
            actor,
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

    /// Cooperatively shut the session down (FR-17 AC).
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
    /// - [`EngineError::ShutdownInProgress`] if a shutdown is in progress (the permit is refused up
    ///   front, before any read/migrate).
    /// - The transparent storage source (`SchemaMismatch`/`Migration`/backend read) on any failure.
    ///
    /// [`StorageError::SchemaMismatch`]: unblock_storage::StorageError::SchemaMismatch
    pub async fn migrate(&self) -> Result<MigrateOutcome> {
        // Write permit (D14), shutdown-aware. `Session::acquire` (write.rs) is private to the `write`
        // module, so — like `interchange.rs` — the lifecycle path calls the crate helper directly.
        let _guard = acquire_write(&self.write_permit, &self.shutdown).await?;
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

    /// Run health/integrity diagnostics (FR-15/FR-16).
    ///
    /// **Pre-T3.3 = SIGNATURE only; body seamed to `unblock-health`.** Returns
    /// [`EngineError::FeatureNotWired`] (`feature: "health"`) and writes nothing until wired.
    /// **T3.3 (HEALTH-LITE, D29) wires the lite body**: it composes `integrity_check()` rows + the pure
    /// file-state classification from `unblock-health` (`run_doctor`) into a `DoctorReport`, then maps it
    /// onto a [`DiagnosticReport`] **reusing the existing `DiagnosticKind::Info`** (F2 — NO new model
    /// variant, no spine §1.10 / `CONTRACT_HASH` change). The cli `doctor` command routes
    /// through this wired `doctor()` from T3.3 (F4). The full 4-state taxonomy + `--repair` are **v1.1**.
    ///
    /// **D46 (v1.0.1) — TWO ADVISORY SCHEMA-VERSION findings are folded in here TOO, and nowhere
    /// else.** `doctor()` additionally awaits the EXISTING pure read
    /// [`Storage::schema_version`](unblock_storage::Storage::schema_version) and inserts the stamp
    /// OBSERVED ON DISK (`schema_version`) then the version THIS BUILD EXPECTS (`schema_expected`), in
    /// that fixed order — **AFTER the file-state anomalies and BEFORE the D45 dangling block, which
    /// REMAINS the report's trailing suffix.** That placement is NORMATIVE (spine §4.1), because the
    /// shipped suffix assertion in `crates/unblock-engine/tests/dangling.rs` is a live D45 mutation
    /// proof: appending after the block would redden a required CI step, and relaxing the suffix to a
    /// subsequence would retire that proof to buy nothing. The findings are ADVISORY — they report two
    /// integers and COMPARE nothing — so `doctor_exit` and the FR-16 exit rule are byte-identical, and
    /// a general schema-conformance check is explicitly out of scope (PRD §4, D46). `unblock-health`
    /// is again NOT touched (D29 clause F3 preserved).
    ///
    /// **D45 — the DANGLING-dependency findings are FOLDED IN HERE, in the ENGINE.** `doctor()`
    /// additionally awaits the SAME fn the `diagnostics {kind:"dangling"}` action uses
    /// ([`crate::diagnostics::dangling_findings`] — ONE home, never a second implementation) and
    /// APPENDS its findings, in the pinned `(issue_id, dep_type, depends_on_id)`
    /// order, AFTER the file-state anomalies (a deterministic overall order, NFR-14). The report's
    /// `kind` stays `Info` — the fold moves no spine §1.10 byte; the `Dangling` KIND exists for the
    /// `diagnostics` tool arm, where the response must declare what it is.
    ///
    /// **`unblock_health::run_doctor` is NOT given a third argument, its signature does NOT change,
    /// and `unblock-health` gains NO `unblock-storage` dependency:** the list is DB-derived and D29
    /// clause F3 makes `run_doctor` PURE, non-async and storage-free — that clause is PRESERVED, not
    /// reversed. Composing in the engine fold is exactly how the engine already folds in the pure
    /// file-state anomalies; passing DB rows into `run_doctor` would reverse a shipped clause.
    ///
    /// **D45 — COST, MEASURED. AMENDED 2026-08-02: the fold is now ONE SQL query.** The fold is
    /// UNCONDITIONAL on every `doctor()` call, which is precisely why its cost mattered. As first
    /// shipped it differenced a whole-graph edge load against a FULLY-INCLUSIVE `list_issues`
    /// (closed + deferred + tombstone) that hydrated labels, dependencies and comments for every row
    /// merely to derive an id SET — `O(rows + edges)` work and `O(rows)` peak memory. At 250k rows
    /// the `scale` gate measured `integrity_check` 5.51 s + the fold 10.72 s = `doctor()` 16.31 s
    /// against a 15 s boundedness guard: a RED required job. It is now ONE
    /// [`Storage::dangling_dependencies`](unblock_storage::Storage::dangling_dependencies) read (a
    /// `LEFT JOIN … WHERE i.id IS NULL` excluding external targets), at the stated price of one trait
    /// method and its implementors. The live numbers are re-derived on every run by the reporting
    /// timings in `crates/unblock-engine/tests/scale.rs`.
    ///
    /// # Errors
    ///
    /// - [`EngineError::FeatureNotWired`] (`feature: "health"`) until the T3.3 wiring lands; thereafter
    ///   the transparent `Health { source: HealthError }` variant on a health failure, or the
    ///   transparent storage source from the D45 dangling read.
    #[cfg_attr(not(feature = "health"), allow(clippy::unused_async))] // no await without health
    pub async fn doctor(&self) -> Result<DiagnosticReport> {
        #[cfg(feature = "health")]
        {
            // Compose the ONE corruption signal reachable now (integrity_check rows) + the pure
            // file-state classification into unblock-health's `DoctorReport`, then fold it onto a
            // `DiagnosticReport` reusing `DiagnosticKind::Info` (F2 — no model change). `run_doctor`
            // is storage-free and non-async (F3); `integrity_check()` (a read, no write permit) is the
            // only await.
            let integrity_rows = self.integrity_check().await?;
            let report = unblock_health::run_doctor(&integrity_rows, &self.health_paths())?;
            let mut diagnostic = doctor_report_to_diagnostic(&report);
            // D46: the TWO ADVISORY schema-version rows — the stamp OBSERVED ON DISK and the version
            // THIS BUILD EXPECTS — over the EXISTING pure `Storage::schema_version()` read (no new
            // trait method, no write permit, no migration side-effect). Their PLACEMENT IS NORMATIVE
            // (spine §4.1, ruled at the 2026-08-03 design gate): AFTER the file-state anomalies and
            // BEFORE the D45 dangling block, which REMAINS the report's trailing suffix — the shipped
            // cell `crates/unblock-engine/tests/dangling.rs` asserts that suffix and its docstring
            // names the mutant it kills, so appending AFTER the block would redden a required CI step
            // and relaxing the suffix to a subsequence would retire a live D45 proof for nothing.
            // They COMPARE nothing (two integers), so the exit rule is byte-identical: `doctor` still
            // exits non-zero only on detected corruption (FR-16). What they buy is that `doctor` can
            // no longer print `healthy` without also printing the number that would contradict it.
            let observed = self.storage.schema_version().await?;
            diagnostic.findings.push(unblock_model::DiagnosticFinding {
                label: "schema_version".to_string(),
                detail: observed.to_string(),
            });
            diagnostic.findings.push(unblock_model::DiagnosticFinding {
                label: "schema_expected".to_string(),
                detail: unblock_storage::CURRENT_SCHEMA_VERSION.to_string(),
            });
            // D45: the dangling-dependency fold. The SAME engine-side fn the `dangling` diagnostics
            // action calls — one home — appended AFTER the file-state anomalies, in the pinned order
            // the read's own `ORDER BY` produces. No write permit: every half is a read (FR-10).
            diagnostic
                .findings
                .extend(crate::diagnostics::dangling_findings(self.storage.as_ref()).await?);
            Ok(diagnostic)
        }
        #[cfg(not(feature = "health"))]
        {
            Err(EngineError::FeatureNotWired { feature: "health" })
        }
    }

    /// Build the [`WorkspacePaths`](unblock_health::WorkspacePaths) bundle `doctor()` classifies over
    /// (F3 — health is storage-free; the engine supplies the already-resolved paths). `recovery_dir`
    /// is reserved for the v1.1 evidence writer (unused by the lite `run_doctor`).
    #[cfg(feature = "health")]
    fn health_paths(&self) -> unblock_health::WorkspacePaths {
        unblock_health::WorkspacePaths {
            db: self.db_path.clone(),
            jsonl: Some(self.jsonl_path.clone()),
            recovery_dir: self.unblock_dir.join(".recovery"),
        }
    }

    /// Attempt workspace repair (WAL checkpoint, reindex; reports actions taken) — FR-16.
    ///
    /// **STAYS SIGNATURE only through v1 (F1/D29).** Returns [`EngineError::FeatureNotWired`]
    /// (`feature: "health"`) and writes nothing. Returns [`DiagnosticReport`] (spine §4.1, NOT a bespoke
    /// `RecoveryReport`); its body — `--repair` (WAL checkpoint/reindex) + the `.unblock/.recovery/`
    /// evidence writer + the rich repair taxonomy — is a **v1.1** deliverable, NOT T3.3. T3.3 wires only
    /// `doctor()` (the read-only lite report); wiring `recover()` to a hollow "nothing repaired" report
    /// would be the faked success `FeatureNotWired` forbids.
    ///
    /// # Errors
    ///
    /// - [`EngineError::FeatureNotWired`] (`feature: "health"`) through v1 (the recover seam is unwired
    ///   until v1.1).
    #[allow(clippy::unused_async)] // async in the spine §4.1 signature; the v1.1 body awaits health repair.
    pub async fn recover(&self) -> Result<DiagnosticReport> {
        Err(EngineError::FeatureNotWired { feature: "health" })
    }
}

/// Map an `unblock-health` [`DoctorReport`](unblock_health::DoctorReport) onto the model
/// [`DiagnosticReport`] `Session::doctor` returns, **reusing the existing
/// [`DiagnosticKind::Info`](unblock_model::DiagnosticKind::Info)** (F2 — no new model variant, no
/// spine §1.10 / `CONTRACT_HASH` change).
///
/// Emits a stable headline (`health` = the composite worst level, `integrity` = `ok`/`N problem(s)`),
/// then one generic [`DiagnosticFinding`](unblock_model::DiagnosticFinding) per integrity problem row
/// and per file-state anomaly (`label` = the anomaly's stable code, `detail` = its `Display`). The
/// order is deterministic (NFR-14 — insta-pinned at the cli/engine boundary).
#[cfg(feature = "health")]
fn doctor_report_to_diagnostic(report: &unblock_health::DoctorReport) -> DiagnosticReport {
    use unblock_model::{DiagnosticFinding, DiagnosticKind};

    let mut findings =
        Vec::with_capacity(2 + report.integrity_rows.len() + report.file_state.len());
    findings.push(DiagnosticFinding {
        label: "health".to_string(),
        detail: report.summary.worst.as_str().to_string(),
    });
    findings.push(DiagnosticFinding {
        label: "integrity".to_string(),
        detail: if report.integrity_ok {
            "ok".to_string()
        } else {
            format!("{} problem(s)", report.integrity_rows.len())
        },
    });
    for row in &report.integrity_rows {
        findings.push(DiagnosticFinding {
            label: "integrity_problem".to_string(),
            detail: row.clone(),
        });
    }
    for anomaly in &report.file_state {
        findings.push(DiagnosticFinding {
            label: anomaly.code().to_string(),
            detail: anomaly.to_string(),
        });
    }
    DiagnosticReport {
        kind: DiagnosticKind::Info,
        findings,
    }
}

#[cfg(all(test, feature = "health"))]
mod tests {
    use super::doctor_report_to_diagnostic;
    use std::path::PathBuf;
    use unblock_health::{WorkspacePaths, run_doctor};
    use unblock_model::DiagnosticKind;

    /// The engine mapping folds corrupt integrity rows into a `recoverable` health finding + one
    /// `integrity_problem` per row, reusing `DiagnosticKind::Info` (F2). Uses non-existent paths so no
    /// file-state anomaly fires and integrity is the sole signal.
    #[test]
    fn corrupt_integrity_maps_to_a_recoverable_info_report() {
        let paths = WorkspacePaths {
            db: PathBuf::from("/nonexistent/unblock.db"),
            jsonl: None,
            recovery_dir: PathBuf::from("/nonexistent/.recovery"),
        };
        let rows = vec![
            "*** in database main ***".to_string(),
            "page 5 is never used".to_string(),
        ];
        let report = run_doctor(&rows, &paths).expect("run_doctor is infallible");
        let diag = doctor_report_to_diagnostic(&report);

        assert_eq!(diag.kind, DiagnosticKind::Info);
        let detail = |label: &str| {
            diag.findings
                .iter()
                .find(|f| f.label == label)
                .map(|f| f.detail.as_str())
        };
        assert_eq!(detail("health"), Some("recoverable"));
        assert_eq!(detail("integrity"), Some("2 problem(s)"));
        let problems: Vec<&str> = diag
            .findings
            .iter()
            .filter(|f| f.label == "integrity_problem")
            .map(|f| f.detail.as_str())
            .collect();
        assert_eq!(problems, rows);
    }
}
