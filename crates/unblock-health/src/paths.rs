//! [`WorkspacePaths`] — the pure path bundle health operates over — plus `SQLite` sidecar derivation.
//!
//! Health does **not** discover `.unblock/` (that is `unblock-config`'s job, L4); the engine builds a
//! [`WorkspacePaths`] from its already-resolved context and passes it in. Sidecar paths use `SQLite`'s
//! own **append-style** naming (`{db}-wal`), NOT `Path::with_extension` (which would corrupt a db
//! path that has no extension, e.g. `unblock` → `unblock-wal` vs a broken `unblock.-wal`, or a
//! multi-dot name).

use std::path::{Path, PathBuf};

/// The `SQLite` write-ahead-log sidecar suffix.
pub const WAL_SUFFIX: &str = "-wal";
/// The `SQLite` shared-memory sidecar suffix.
pub const SHM_SUFFIX: &str = "-shm";
/// The `SQLite` rollback-journal sidecar suffix.
pub const JOURNAL_SUFFIX: &str = "-journal";

/// The bundle of workspace paths health inspects, supplied by the engine (health never discovers
/// `.unblock/` itself — it sits at L3, below config's L4).
#[derive(Debug, Clone)]
pub struct WorkspacePaths {
    /// The libsql database file.
    pub db: PathBuf,
    /// The JSONL export file, or `None` when export is not configured.
    pub jsonl: Option<PathBuf>,
    /// The recovery-evidence directory (`.unblock/.recovery/`). Reserved for the v1.1 evidence
    /// writer; unused by v1-lite, but carried so the v1.1 layout needs no signature change.
    pub recovery_dir: PathBuf,
}

/// Derive a `SQLite` sidecar path by **appending** `suffix` to the db path's raw bytes (`{db}{suffix}`),
/// matching `SQLite`'s own `-wal`/`-shm`/`-journal` naming.
///
/// Uses [`OsString`](std::ffi::OsString) concatenation rather than string formatting so a non-UTF-8
/// db path is preserved byte-for-byte (a strict improvement over the original's lossy
/// `to_string_lossy`); the result is identical for every valid UTF-8 path.
#[must_use]
pub fn sidecar(db: &Path, suffix: &str) -> PathBuf {
    let mut raw = db.as_os_str().to_os_string();
    raw.push(suffix);
    PathBuf::from(raw)
}

#[cfg(test)]
mod tests {
    use super::{JOURNAL_SUFFIX, SHM_SUFFIX, WAL_SUFFIX, sidecar};
    use std::path::{Path, PathBuf};

    #[test]
    fn sidecar_appends_to_a_dotted_db_name() {
        let db = Path::new("/ws/.unblock/unblock.db");
        assert_eq!(
            sidecar(db, WAL_SUFFIX),
            PathBuf::from("/ws/.unblock/unblock.db-wal")
        );
        assert_eq!(
            sidecar(db, SHM_SUFFIX),
            PathBuf::from("/ws/.unblock/unblock.db-shm")
        );
        assert_eq!(
            sidecar(db, JOURNAL_SUFFIX),
            PathBuf::from("/ws/.unblock/unblock.db-journal")
        );
    }

    #[test]
    fn sidecar_appends_to_an_extensionless_db_name() {
        // `with_extension("-wal")` would produce a WRONG `unblock-wal`-as-extension; append is right.
        let db = Path::new("/ws/unblock");
        assert_eq!(sidecar(db, WAL_SUFFIX), PathBuf::from("/ws/unblock-wal"));
    }

    #[test]
    fn sidecar_appends_to_a_multi_dot_db_name() {
        let db = Path::new("issues.sqlite");
        assert_eq!(sidecar(db, SHM_SUFFIX), PathBuf::from("issues.sqlite-shm"));
    }

    #[test]
    fn sidecar_is_pure_no_side_effects() {
        // A relative, non-existent path derives without touching the filesystem.
        let db = Path::new("does/not/exist.db");
        assert_eq!(
            sidecar(db, WAL_SUFFIX),
            PathBuf::from("does/not/exist.db-wal")
        );
    }
}
