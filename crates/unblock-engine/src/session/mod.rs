//! The [`Session`] struct (the single shared state) + [`SessionConfig`], with the per-concern
//! `impl Session` modules (lifecycle / read / write / interchange).
//!
//! `Session` is the single mutation home (FR-9): MCP and CLI are thin adapters over this surface, so
//! behaviour cannot drift. Writes serialize through one `Arc<tokio::sync::Semaphore>` with **one
//! permit** (D14, spine §4.2); reads bypass the permit (FR-10). The struct is `Send + Sync + 'static`
//! so it can be shared across MCP tasks.
//!
//! **NO `policy` field** (OQ-1 RESOLVED): `unblock-policy` ships only free functions — `ready()`
//! calls `unblock_policy::cmp_ready` directly and `close_with_suggestions` consumes `is_ready` as a
//! free fn. `sync`/`health` are typed not-wired seams (no handle field in v1).

pub(crate) mod ids;
pub(crate) mod interchange;
pub(crate) mod lifecycle;
pub(crate) mod read;
pub(crate) mod write;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::Semaphore;
use unblock_config::ResolvedConfig;
use unblock_storage::Storage;

/// Engine-behaviour knobs passed to [`Session::open`] (spine §4.1).
///
/// Post-CF-D, `workspace_dir`/`actor`/the resolved config arrive via the `WorkspaceContext`
/// (config-owned) and are **no longer** here. `SessionConfig` carries only the bool engine knobs —
/// **verbatim per spine §4.1**. `Default` = all-false.
#[derive(Debug, Clone, Default)]
pub struct SessionConfig {
    /// Auto-export JSONL after mutating ops (FR-7). *(v1: the export path is the sync seam, T2.4.)*
    pub jsonl_export: bool,
    /// Run the import seam during `open()` if set (FR-8). *(v1: delegates to the sync seam, T2.4 —
    /// `open(import_on_open=true)` returns `FeatureNotWired{"sync"}` until then.)*
    pub import_on_open: bool,
    /// Enable the non-default remote storage path (D15; off in v1).
    pub remote: bool,
}

/// The single mutation home (FR-9) — composes storage + policy (+ optional sync/health) over one
/// lifecycle, serializing writes through a `Semaphore(1)` (D14) while reads bypass it (FR-10).
///
/// Built by [`Session::open`] from a `WorkspaceContext` (config builds the `Arc<dyn Storage>`, CF-D;
/// the engine never constructs storage). `Send + Sync + 'static`.
pub struct Session {
    /// The backend handle, received from the `WorkspaceContext` (NOT built here, CF-D).
    pub(crate) storage: Arc<dyn Storage>,
    /// The single write permit (1 permit, D14, spine §4.2). Reads never touch it (FR-10).
    pub(crate) write_permit: Arc<Semaphore>,
    /// The resolved config VALUES the session reads (config-owned, from the context).
    pub(crate) config: ResolvedConfig,
    /// The authoritative actor (spine §4.1, from the context).
    pub(crate) actor: String,
    /// The project root (the dir that contains `.unblock/`, from the context).
    pub(crate) workspace_dir: PathBuf,
    /// The discovered `.unblock/` directory (from the context's `ConfigPaths`).
    pub(crate) unblock_dir: PathBuf,
    /// The libsql database path (from the context's `ConfigPaths`).
    pub(crate) db_path: PathBuf,
    /// The JSONL export path (from the context's `ConfigPaths`).
    pub(crate) jsonl_path: PathBuf,
    /// The engine-behaviour knobs (spine §4.1; `SessionConfig`).
    pub(crate) knobs: SessionConfig,
    /// The cooperative shutdown flag (FR-17, OQ-4 — installed by the cli; the engine only reads/sets
    /// it via `shutdown()`). Wrapped in `Arc` so a cli-installed flag can be shared.
    pub(crate) shutdown: Arc<AtomicBool>,
}

impl Session {
    /// Wire a cli-installed cooperative-shutdown flag into this session (FR-17, OQ-4).
    ///
    /// The OS signal handler that *sets* the flag lives in `unblock-cli` (a library must not hijack
    /// process-global signals); this lets the cli hand the `Arc<AtomicBool>` it owns to the engine,
    /// which then reads it at mutation checkpoints. Returns the session for chaining after `open`.
    #[must_use]
    pub fn with_shutdown_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.shutdown = flag;
        self
    }

    /// Read whether a cooperative shutdown has been requested (FR-17).
    ///
    /// Reads the wired flag (the cli installs the OS handler that sets it, OQ-4); `false` by default.
    #[must_use]
    pub fn is_shutdown_requested(&self) -> bool {
        crate::shutdown::is_shutdown_requested(&self.shutdown)
    }

    /// The authoritative actor for this session (spine §4.1).
    #[must_use]
    pub fn actor(&self) -> &str {
        &self.actor
    }

    /// The project root (the directory that contains `.unblock/`).
    #[must_use]
    pub fn workspace_dir(&self) -> &std::path::Path {
        &self.workspace_dir
    }
}

#[cfg(test)]
mod tests {
    use super::{Session, SessionConfig};

    const fn assert_send_sync<T: Send + Sync + 'static>() {}

    #[test]
    fn session_is_send_sync_static() {
        assert_send_sync::<Session>();
    }

    #[test]
    fn session_config_default_is_all_false() {
        let cfg = SessionConfig::default();
        assert!(!cfg.jsonl_export);
        assert!(!cfg.import_on_open);
        assert!(!cfg.remote);
    }

    #[test]
    fn session_config_round_trips_debug_clone() {
        let cfg = SessionConfig {
            jsonl_export: true,
            import_on_open: false,
            remote: false,
        };
        let cloned = cfg.clone();
        assert_eq!(cloned.jsonl_export, cfg.jsonl_export);
        assert!(format!("{cfg:?}").contains("jsonl_export"));
    }
}
