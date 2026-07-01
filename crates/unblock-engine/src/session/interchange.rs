//! `impl Session` JSONL/bd interchange (FR-7/8/26).
//!
//! `export_jsonl`/`import_jsonl` delegate to `unblock-sync` (L3) behind the default-on `sync`
//! feature (T2.4). Under `--no-default-features` the bodies fall back to the typed
//! [`EngineError::FeatureNotWired`] (`feature: "sync"`) so the methods compile with OR without the
//! feature. `import_bd` STAYS `FeatureNotWired{"sync"}` under both cfgs until T2.5.
//!
//! **Never a faked success and never inline JSONL logic in the engine** — atomic write /
//! conflict-marker preflight / path confinement / tombstone-no-resurrect all live in `unblock-sync`
//! at L3. `import_jsonl` acquires the D14 write permit (MF-4) across the sync call; `export_jsonl` is
//! read-only and takes NO permit.

use std::path::Path;

use crate::error::{EngineError, Result};
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
        let sync_opts = unblock_sync::ImportOptions {
            dry_run: opts.dry_run,
            allow_external: false,
            on_collision: unblock_sync::CollisionPolicy::Skip,
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

    /// One-shot, idempotent `bd` import (idempotent via `content_hash`) — D16/FR-26.
    ///
    /// **STAYS seam-deferred to T2.5** under both cfgs: the bd field-map is not built at T2.4, so
    /// this returns the typed [`EngineError::FeatureNotWired`] (`feature: "sync"`) and writes nothing.
    ///
    /// # Errors
    /// - [`EngineError::FeatureNotWired`] (`feature: "sync"`) in v1 — writes nothing.
    #[allow(clippy::unused_async)] // async in the spine §4.1 signature; the T2.5 body awaits sync.
    pub async fn import_bd(&self, _path: &Path) -> Result<ImportReport> {
        Err(EngineError::FeatureNotWired { feature: "sync" })
    }
}
