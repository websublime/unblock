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

    /// Ensure the schema is at the current baseline, idempotently, and report the from→to delta
    /// (D27/AF-2, T3.1 — spine §4.1).
    ///
    /// Runs **under the single write permit** (D14 — migration is a write-path op): it reads
    /// `from = storage.schema_version()` under the held permit (a consistent snapshot no interleaved
    /// writer can advance), runs the idempotent `storage.migrate()` (a no-op on a current DB), re-reads
    /// `to`, and returns `MigrateOutcome { from, to, applied: from != to }`. A database stamped at a
    /// version NEWER than this build surfaces the transparent [`StorageError::SchemaMismatch`] (→ exit
    /// 2) — never a fake success. Because the config open facade migrates on open (FR-9 single open
    /// path), `applied` is normally `false` post-open. Backs the cli `migrate` command.
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
    /// variant, no spine §1.10 / `CONTRACT_HASH` change; the landed `DiagnosticKind` set —
    /// `Stats|Info|Where|Version|Lint|Changelog|Orphans` — is unchanged). The cli `doctor` command routes
    /// through this wired `doctor()` from T3.3 (F4). The full 4-state taxonomy + `--repair` are **v1.1**.
    ///
    /// # Errors
    ///
    /// - [`EngineError::FeatureNotWired`] (`feature: "health"`) until the T3.3 wiring lands; thereafter
    ///   the transparent `Health { source: HealthError }` variant on a health failure.
    #[allow(clippy::unused_async)] // async in the spine §4.1 signature; the T3.3 body awaits health.
    pub async fn doctor(&self) -> Result<DiagnosticReport> {
        Err(EngineError::FeatureNotWired { feature: "health" })
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
