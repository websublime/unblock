//! [`WorkspacePaths`] — the pure path bundle health operates over.
//!
//! Health does **not** discover `.unblock/` (that is `unblock-config`'s job, L4); the engine builds a
//! [`WorkspacePaths`] from its already-resolved context and passes it in. The `SQLite` sidecar
//! suffixes and `sidecar()` live in `unblock-sync`, their owner above L2 (PRD §4 D5 amendment,
//! `ub-c15`); this crate re-exports them from its root.

use std::path::PathBuf;

/// The bundle of workspace paths health inspects, supplied by the engine (health never discovers
/// `.unblock/` itself — it sits at L3, below config's L4).
#[derive(Debug, Clone)]
pub struct WorkspacePaths {
    /// The libsql database file.
    pub db: PathBuf,
    /// The JSONL export file, or `None` when export is not configured.
    pub jsonl: Option<PathBuf>,
    /// The recovery-evidence root (`.unblock/.recovery/`), the root
    /// [`RunDir::create`](crate::recovery::RunDir::create) and
    /// [`scan_recovery_root`](crate::recovery::scan_recovery_root) take.
    pub recovery_dir: PathBuf,
}
