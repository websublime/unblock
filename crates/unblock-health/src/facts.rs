//! The four [`classify`](crate::classify) inputs (D58 clause (4)).
//!
//! Every type here is plain data. The engine gathers [`PreOpenFacts`] with `std::fs` before the
//! process opens any libsql connection, maps `Storage::health_facts()` into [`DbFacts`], builds
//! [`ExportFacts`] through [`scan_export`](crate::scan_export) and injects the clock through
//! [`AdvisoryFacts`]. No storage type crosses into health.

use std::path::PathBuf;
use std::time::SystemTime;

/// What a plain read of the database file's first 16 bytes found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbFileFact {
    /// No file exists at the configured database path.
    Absent,
    /// The file exists and holds 0 bytes.
    Empty,
    /// The file exists but its first bytes cannot be read (a permission error, for example).
    Unreadable {
        /// The I/O error text.
        reason: String,
    },
    /// The file holds 1 byte or more and its first 16 bytes are not the `SQLite` magic.
    NotSqlite {
        /// The file length in bytes.
        len: u64,
    },
    /// The file starts with the 16-byte `SQLite` magic.
    SqliteHeader,
}

/// The pre-open file facts (D58 clauses (2), (6), (7) and (9)).
#[allow(clippy::struct_excessive_bools)] // one flag per independent file fact (spine §3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreOpenFacts {
    /// The database file's header read.
    pub db: DbFileFact,
    /// `unblock_storage::lost_database`'s answer: the file is absent or 0 bytes while
    /// `.unblock/.write.lock` exists.
    pub lost: bool,
    /// The `-wal` sidecar's length, or `None` when it is absent.
    pub wal_len: Option<u64>,
    /// Whether a `-shm` sidecar exists.
    pub shm_present: bool,
    /// Whether a rollback `-journal` sidecar exists.
    pub journal_present: bool,
    /// Whether another process holds `.unblock/.write.lock` right now.
    pub write_lock_held: bool,
    /// The run directories under `.unblock/.recovery/`.
    pub recovery_entries: Vec<RecoveryEntry>,
}

/// One run directory under `.unblock/.recovery/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryEntry {
    /// The directory name.
    pub name: String,
    /// The UTC instant its `YYYYMMDDTHHMMSSZ-<run-id>` name encodes, or `None` when the name does
    /// not parse (its age is unknown).
    pub stamp_utc: Option<SystemTime>,
}

/// The database facts the engine maps from `Storage::health_facts()`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DbFacts {
    /// The rows of one `PRAGMA integrity_check` (`ok` normalized to no rows).
    pub integrity_rows: Vec<String>,
    /// The schema stamp observed on disk (`PRAGMA user_version`).
    pub schema_version: i64,
    /// The schema version this build expects.
    pub expected_schema_version: i64,
    /// The index names found in `sqlite_schema`.
    pub index_names: Vec<String>,
    /// `(parent_id, stored, observed_max)` for each `child_counters` row below its greatest child.
    pub child_counter_drift: Vec<(String, u32, u32)>,
    /// `(key, rows)` for each `metadata` key held by two or more rows.
    pub duplicate_metadata_keys: Vec<(String, u32)>,
}

/// The export scan result, with the path it was taken from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportFacts {
    /// The export path, or `None` when no export is configured.
    pub path: Option<PathBuf>,
    /// What the conflict-marker scan found.
    pub scan: ExportScan,
}

/// What `unblock_sync::scan_conflict_markers` found in the export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportScan {
    /// No export is configured, or the file does not exist.
    Absent,
    /// The export carries no conflict marker.
    Clean,
    /// The export carries git conflict markers.
    ConflictMarkers {
        /// The 1-based line of the first marker.
        first_line: usize,
    },
    /// The scanner refused the file with an I/O, file-size or line-length guard error.
    Unscannable {
        /// The scanner's error text.
        reason: String,
    },
    /// The export path is the session database or one of its `-wal`/`-shm`/`-journal` sidecars (same
    /// device and inode, under any name, a hard link or a symlink included; PRD §4 D5 amendment,
    /// `ub-c15`). It was not scanned: a path that stats as one is never opened, and one recognized
    /// only on its opened descriptor is kept open, never closed.
    SessionFile,
}

/// The inputs of the advisories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdvisoryFacts {
    /// The current UTC instant, injected so the 30-day stale rule is testable.
    pub now_utc: SystemTime,
}
