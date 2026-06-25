//! `impl Session` JSONL/bd interchange (FR-7/8/26) — **v1 SEAM-DEFERRED (typed not-wired)**.
//!
//! All three method **signatures land now** (so L7 binds to a stable surface), but their bodies
//! delegate to `unblock-sync` (T2.4) — still a stub — so each returns the typed
//! [`EngineError::FeatureNotWired`] (`feature: "sync"`) until T2.4. **Never a faked success and never
//! inline JSONL logic in the engine** (atomic write / conflict-marker preflight / path confinement /
//! tombstone-no-resurrect all live in `unblock-sync` at L3; inlining them would breach the L3
//! boundary and FR-9 single-path).
//!
//! The bodies deliberately **do not reference** the (optional, default-on) `unblock-sync` crate, so
//! they compile identically with **or without** the `sync` feature — i.e. `cargo build
//! --no-default-features` builds these methods unchanged. T2.4 replaces the bodies with the real
//! delegation (gating the real call behind `#[cfg(feature = "sync")]`), purely additively.

use std::path::Path;

use crate::error::{EngineError, Result};
use crate::report::{ExportReport, ImportOptions, ImportReport};
use crate::session::Session;

impl Session {
    /// Export the store to JSONL atomically (temp+fsync+rename, NFR-4) — FR-7.
    ///
    /// **v1 = SIGNATURE only; body seamed to `unblock-sync` (T2.4).**
    ///
    /// # Errors
    /// - [`EngineError::FeatureNotWired`] (`feature: "sync"`) in v1 — writes nothing.
    #[allow(clippy::unused_async)] // async in the spine §4.1 signature; the T2.4 body awaits sync.
    pub async fn export_jsonl(&self, _path: &Path) -> Result<ExportReport> {
        Err(EngineError::FeatureNotWired { feature: "sync" })
    }

    /// Import JSONL (preflight before any DB mutation, then apply under the write permit) — FR-8.
    ///
    /// **v1 = SIGNATURE only; body seamed to `unblock-sync` (T2.4).** The public `opts:
    /// ImportOptions { dry_run }` is the spine-owned type; T2.4 maps it into sync's internal options.
    ///
    /// # Errors
    /// - [`EngineError::FeatureNotWired`] (`feature: "sync"`) in v1 — writes nothing.
    #[allow(clippy::unused_async)] // async in the spine §4.1 signature; the T2.4 body awaits sync.
    pub async fn import_jsonl(&self, _path: &Path, _opts: ImportOptions) -> Result<ImportReport> {
        Err(EngineError::FeatureNotWired { feature: "sync" })
    }

    /// One-shot, idempotent `bd` import (idempotent via `content_hash`) — D16/FR-26.
    ///
    /// **v1 = SIGNATURE only; body seamed to `unblock-sync` (T2.4).**
    ///
    /// # Errors
    /// - [`EngineError::FeatureNotWired`] (`feature: "sync"`) in v1 — writes nothing.
    #[allow(clippy::unused_async)] // async in the spine §4.1 signature; the T2.4 body awaits sync.
    pub async fn import_bd(&self, _path: &Path) -> Result<ImportReport> {
        Err(EngineError::FeatureNotWired { feature: "sync" })
    }
}
