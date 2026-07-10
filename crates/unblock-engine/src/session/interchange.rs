//! `impl Session` JSONL/bd interchange (FR-7/8/26).
//!
//! `export_jsonl`/`import_jsonl`/`import_bd` delegate to `unblock-sync` (L3) behind the default-on
//! `sync` feature (T2.4/T2.5). Under `--no-default-features` the bodies fall back to the typed
//! [`EngineError::FeatureNotWired`] (`feature: "sync"`) so the methods compile with OR without the
//! feature.
//!
//! **Never a faked success and never inline JSONL logic in the engine** — atomic write /
//! conflict-marker preflight / path confinement / tombstone-no-resurrect / the bd import-normalize
//! repair pass all live in `unblock-sync` at L3. `import_jsonl`/`import_bd` acquire the D14 write
//! permit (MF-4) across the sync call; `export_jsonl` is read-only and takes NO permit.

use std::path::Path;

#[cfg(not(feature = "sync"))]
use crate::error::EngineError;
use crate::error::Result;
use crate::report::{ExportReport, ImportOptions, ImportReport};
use crate::session::Session;

impl Session {
    /// Export the store to JSONL atomically (temp+fsync+rename, NFR-4) — FR-7.
    ///
    /// Read-only: pulls a snapshot of the full non-ephemeral corpus (incl. closed + tombstones) and
    /// writes it atomically. Takes NO write permit (export never mutates).
    ///
    /// # Errors
    /// - The transparent [`EngineError::Sync`] source on a path/serialization/I/O failure (sync-on).
    /// - [`EngineError::FeatureNotWired`] (`feature: "sync"`) under `--no-default-features`.
    #[cfg(feature = "sync")]
    pub async fn export_jsonl(&self, path: &Path) -> Result<ExportReport> {
        let report = unblock_sync::export_jsonl(
            &*self.storage,
            path,
            &self.unblock_dir,
            &unblock_sync::ExportOptions {
                allow_external: false,
                // NFR-5/D30 forward seam: v1 never sets `allow_external`, so no reason is threaded.
                external_reason: None,
            },
        )
        .await?;
        Ok(report)
    }

    /// Export the store to JSONL atomically (temp+fsync+rename, NFR-4) — FR-7.
    ///
    /// # Errors
    /// - [`EngineError::FeatureNotWired`] (`feature: "sync"`) — writes nothing.
    #[cfg(not(feature = "sync"))]
    #[allow(clippy::unused_async)] // async in the spine §4.1 signature; the sync body awaits.
    pub async fn export_jsonl(&self, _path: &Path) -> Result<ExportReport> {
        Err(EngineError::FeatureNotWired { feature: "sync" })
    }

    /// Import JSONL (preflight before any DB mutation, then apply under the write permit) — FR-8.
    ///
    /// Acquires the single D14 write permit for the WHOLE call (MF-4) — the sync classify probes and
    /// the atomic `create_issues` tx run under it, so no concurrent writer races a classified-new id.
    /// `dry_run` still takes the permit for a consistent snapshot. The engine maps its public
    /// [`ImportOptions`] into sync's internal options at the call site (spine §4.1).
    ///
    /// # Errors
    /// - The transparent [`EngineError::Sync`] source on a preflight/apply failure (sync-on).
    /// - [`EngineError::ShutdownInProgress`] if a shutdown is in progress.
    /// - [`EngineError::FeatureNotWired`] (`feature: "sync"`) under `--no-default-features`.
    #[cfg(feature = "sync")]
    pub async fn import_jsonl(&self, path: &Path, opts: ImportOptions) -> Result<ImportReport> {
        // MF-4: hold the D14 write permit across the whole sync call (classify probes + tx).
        let _guard = crate::permit::acquire_write(&self.write_permit, &self.shutdown).await?;
        // D31: also hold the cross-process `.write.lock` across the whole import (classify probes +
        // the atomic `create_issues` tx) so a concurrent serve on another process cannot interleave.
        // Declared AFTER the permit, so it drops FIRST (release-inner-first, spine §4.2).
        let _lock = self.storage.acquire_write_lock().await?;
        let sync_opts = unblock_sync::ImportOptions {
            dry_run: opts.dry_run,
            allow_external: false,
            on_collision: unblock_sync::CollisionPolicy::Skip,
            // NFR-5/D30 forward seam: v1 never sets `allow_external`, so no reason is threaded.
            external_reason: None,
        };
        let report = unblock_sync::import_jsonl(
            &*self.storage,
            path,
            &self.unblock_dir,
            self.actor(),
            &sync_opts,
        )
        .await?;
        Ok(report)
    }

    /// Import JSONL (preflight before any DB mutation, then apply under the write permit) — FR-8.
    ///
    /// # Errors
    /// - [`EngineError::FeatureNotWired`] (`feature: "sync"`) — writes nothing.
    #[cfg(not(feature = "sync"))]
    #[allow(clippy::unused_async)] // async in the spine §4.1 signature; the sync body awaits.
    pub async fn import_jsonl(&self, _path: &Path, _opts: ImportOptions) -> Result<ImportReport> {
        Err(EngineError::FeatureNotWired { feature: "sync" })
    }

    /// One-shot, best-effort `bd` import (idempotent via `content_hash`) — D16/FR-26/D24.
    ///
    /// Acquires the single D14 write permit for the WHOLE call (MF-4) — the bd import-normalize repair
    /// pass runs at L3, then the shared atomic classify+`create_issues` tail runs under the permit so
    /// no concurrent writer races a classified-new id. Skip-only production semantics (bd ids
    /// preserved verbatim; unknown top-level fields reported in `dropped_fields`;
    /// `dependencies`/`comments` counts are the POST-repair/POST-dedup relation tallies).
    ///
    /// # Errors
    /// - The transparent [`EngineError::Sync`] source on a preflight/map/apply failure (sync-on).
    /// - [`EngineError::ShutdownInProgress`] if a shutdown is in progress.
    /// - [`EngineError::FeatureNotWired`] (`feature: "sync"`) under `--no-default-features`.
    #[cfg(feature = "sync")]
    pub async fn import_bd(&self, path: &Path) -> Result<ImportReport> {
        // MF-4: hold the D14 write permit across the whole sync call (map + classify probes + tx).
        let _guard = crate::permit::acquire_write(&self.write_permit, &self.shutdown).await?;
        // D31: also hold the cross-process `.write.lock` across the whole bd import (declared after
        // the permit so it drops first — release-inner-first, spine §4.2).
        let _lock = self.storage.acquire_write_lock().await?;
        let report =
            unblock_sync::import_bd(&*self.storage, path, &self.unblock_dir, self.actor()).await?;
        Ok(report)
    }

    /// One-shot, best-effort `bd` import (idempotent via `content_hash`) — D16/FR-26/D24.
    ///
    /// # Errors
    /// - [`EngineError::FeatureNotWired`] (`feature: "sync"`) — writes nothing.
    #[cfg(not(feature = "sync"))]
    #[allow(clippy::unused_async)] // async in the spine §4.1 signature; the sync body awaits.
    pub async fn import_bd(&self, _path: &Path) -> Result<ImportReport> {
        Err(EngineError::FeatureNotWired { feature: "sync" })
    }
}
