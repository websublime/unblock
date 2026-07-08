//! v1-lite file-state diagnostics — **pure filesystem inspection, no DB handle** (a ground-truth port
//! of the original `classify_file_state` + `jsonl_has_conflict_markers` + `is_orphaned_lock_file`,
//! `temp/beads_rust-main/src/health.rs`, scoped to the v1 subset).
//!
//! The severity map (D29-F5, ratified): every file-state anomaly is [`Recoverable`] EXCEPT
//! [`JsonlConflictMarkers`], which is [`Unsafe`] (a merge conflict left in the JSONL is not safely
//! auto-recoverable). Deviations from the original are ONLY those D29 ratifies: the lock file is
//! renamed `.unblock.lock` (D8) and the severities live on [`HealthLevel`] (`Degraded` → `Drifted`,
//! though no v1-lite anomaly is `Drifted`).
//!
//! [`Recoverable`]: HealthLevel::Recoverable
//! [`Unsafe`]: HealthLevel::Unsafe
//! [`JsonlConflictMarkers`]: FileAnomaly::JsonlConflictMarkers

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use schemars::JsonSchema;
use serde::Serialize;

use crate::level::HealthLevel;
use crate::paths::{self, JOURNAL_SUFFIX, SHM_SUFFIX, WAL_SUFFIX};

/// A lock file older than this is considered orphaned (30 minutes) — faithful to the original.
const ORPHANED_LOCK_FILE_STALE_AFTER: Duration = Duration::from_mins(30);
/// The conflict-marker sigil length — every git marker line begins with 7 identical sigil bytes
/// followed by a space (`<<<<<<< `, `>>>>>>> `, `======= `, `||||||| `).
const CONFLICT_MARKER_PREFIX_LEN: usize = 7;
/// The 16-byte `SQLite` magic header a valid database file starts with.
const SQLITE_MAGIC: [u8; 16] = *b"SQLite format 3\0";
/// The size of a valid WAL header. A NON-EMPTY WAL smaller than this is truncated (a crash
/// signature); an EMPTY (0-byte) WAL is a valid state (see [`wal_is_truncated`]).
const MIN_WAL_LEN: u64 = 32;
/// The workspace lock file name (D8 — renamed from the original `.beads.lock`).
const LOCK_FILE_NAME: &str = ".unblock.lock";

/// A v1-lite file-state finding — exactly the seven on-disk anomalies the pure classifier detects.
///
/// Each carries a stable string [`code`](Self::code), a [`severity`](Self::severity), and a
/// human [`Display`](Self::fmt); the classifier pushes them in a **fixed deterministic order**
/// (variant-declaration order) so snapshots stay stable (NFR-14). The declaration order is the
/// **faithful beads push/render order** (`temp/beads_rust-main/src/health.rs`): `JsonlConflictMarkers`
/// sits at **position 5**, BEFORE `JournalSidecarPresent` and `OrphanedLockFile`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileAnomaly {
    /// The database file is absent while a JSONL export exists (recoverable by re-import).
    DatabaseMissing,
    /// The database file is present but its header is not the `SQLite` magic.
    DatabaseNotSqlite,
    /// The `-shm` sidecar exists without a `-wal` sidecar (an inconsistent WAL state).
    SidecarMismatch {
        /// Whether the `-wal` sidecar is present.
        has_wal: bool,
        /// Whether the `-shm` sidecar is present.
        has_shm: bool,
    },
    /// The `-wal` sidecar is non-empty but smaller than its 32-byte header (a truncated/crashed WAL;
    /// an empty 0-byte WAL is a valid live/checkpointed state and is not flagged).
    TruncatedWal,
    /// The JSONL export contains git merge-conflict markers.
    JsonlConflictMarkers,
    /// A rollback `-journal` sidecar is present (an incomplete transaction).
    JournalSidecarPresent,
    /// An orphaned, stale `.unblock.lock` file is present.
    OrphanedLockFile,
}

impl FileAnomaly {
    /// The stable string code for this anomaly (used as a diagnostic-finding label).
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::DatabaseMissing => "database_missing",
            Self::DatabaseNotSqlite => "database_not_sqlite",
            Self::SidecarMismatch { .. } => "sidecar_mismatch",
            Self::TruncatedWal => "truncated_wal",
            Self::JsonlConflictMarkers => "jsonl_conflict_markers",
            Self::JournalSidecarPresent => "journal_sidecar_present",
            Self::OrphanedLockFile => "orphaned_lock_file",
        }
    }

    /// The severity this anomaly contributes to the composite health level (D29-F5).
    ///
    /// [`JsonlConflictMarkers`](Self::JsonlConflictMarkers) is `Unsafe`; every other v1-lite anomaly
    /// is `Recoverable`.
    #[must_use]
    pub fn severity(&self) -> HealthLevel {
        match self {
            Self::JsonlConflictMarkers => HealthLevel::Unsafe,
            Self::DatabaseMissing
            | Self::DatabaseNotSqlite
            | Self::SidecarMismatch { .. }
            | Self::TruncatedWal
            | Self::JournalSidecarPresent
            | Self::OrphanedLockFile => HealthLevel::Recoverable,
        }
    }
}

impl fmt::Display for FileAnomaly {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DatabaseMissing => f.write_str("database file missing"),
            Self::DatabaseNotSqlite => f.write_str("database file is not SQLite"),
            Self::SidecarMismatch { has_wal, has_shm } => {
                write!(f, "sidecar mismatch (WAL={has_wal}, SHM={has_shm})")
            }
            Self::TruncatedWal => f.write_str("truncated WAL sidecar (<32 bytes)"),
            Self::JsonlConflictMarkers => f.write_str("JSONL contains merge conflict markers"),
            Self::JournalSidecarPresent => {
                f.write_str("journal sidecar present (incomplete transaction)")
            }
            Self::OrphanedLockFile => f.write_str("orphaned lock file (.unblock.lock) present"),
        }
    }
}

/// Classify the on-disk state of a workspace into a deterministic [`Vec`] of [`FileAnomaly`].
///
/// **Pure** — inspects the filesystem only (no DB handle, no writes). `jsonl` is `None` when JSONL
/// export is not configured (then no JSONL-derived anomaly can fire). Findings are pushed in
/// variant-declaration order (fixed for snapshot stability, NFR-14). Firing conditions are a faithful
/// port of the original:
/// - [`DatabaseMissing`](FileAnomaly::DatabaseMissing): the db is NOT a file AND the jsonl IS a file.
/// - [`DatabaseNotSqlite`](FileAnomaly::DatabaseNotSqlite): the db opens but its first 16 bytes are
///   not the `SQLite` magic (or are shorter than 16 bytes). An open FAILURE is conservatively NOT
///   flagged.
/// - [`SidecarMismatch`](FileAnomaly::SidecarMismatch): `has_shm && !has_wal` only (a lone `-wal` is
///   normal for libsql).
/// - [`TruncatedWal`](FileAnomaly::TruncatedWal): `has_wal` and the `-wal` file is non-empty but
///   `< 32` bytes (an empty 0-byte WAL is valid — see [`wal_is_truncated`]).
/// - [`JsonlConflictMarkers`](FileAnomaly::JsonlConflictMarkers): the jsonl is a file and contains a
///   git conflict marker (see [`jsonl_has_conflict_markers`]).
/// - [`JournalSidecarPresent`](FileAnomaly::JournalSidecarPresent): a `-journal` sidecar exists.
/// - [`OrphanedLockFile`](FileAnomaly::OrphanedLockFile): a `.unblock.lock` exists next to the db and
///   is stale (see [`is_orphaned_lock_file`]).
#[must_use]
pub fn classify_file_state(db: &Path, jsonl: Option<&Path>) -> Vec<FileAnomaly> {
    let mut anomalies = Vec::new();

    let db_is_file = db.is_file();

    // DatabaseMissing: the db is gone but a JSONL export survives to recover from.
    if !db_is_file && jsonl.is_some_and(Path::is_file) {
        anomalies.push(FileAnomaly::DatabaseMissing);
    }

    // DatabaseNotSqlite: the db is present but its header is not the SQLite magic.
    if db_is_file && db_header_mismatches_sqlite(db) {
        anomalies.push(FileAnomaly::DatabaseNotSqlite);
    }

    let wal_path = paths::sidecar(db, WAL_SUFFIX);
    let shm_path = paths::sidecar(db, SHM_SUFFIX);
    let has_wal = wal_path.is_file();
    let has_shm = shm_path.is_file();

    // SidecarMismatch: an `-shm` without its `-wal` (a lone `-wal` is normal for libsql).
    if has_shm && !has_wal {
        anomalies.push(FileAnomaly::SidecarMismatch { has_wal, has_shm });
    }

    // TruncatedWal: a `-wal` smaller than its 32-byte header (a crash signature).
    if has_wal && wal_is_truncated(&wal_path) {
        anomalies.push(FileAnomaly::TruncatedWal);
    }

    // JsonlConflictMarkers: unresolved merge markers in the JSONL export. Pushed at position 5 —
    // BEFORE journal + orphaned-lock — to preserve the faithful beads push/render order (NFR-14).
    if jsonl.is_some_and(|p| p.is_file() && jsonl_has_conflict_markers(p)) {
        anomalies.push(FileAnomaly::JsonlConflictMarkers);
    }

    // JournalSidecarPresent: a rollback `-journal` (an incomplete transaction).
    if paths::sidecar(db, JOURNAL_SUFFIX).is_file() {
        anomalies.push(FileAnomaly::JournalSidecarPresent);
    }

    // OrphanedLockFile: a stale `.unblock.lock` next to the db.
    let lock_path = lock_file_path(db);
    if lock_path.is_file() && is_orphaned_lock_file(&lock_path, SystemTime::now()) {
        anomalies.push(FileAnomaly::OrphanedLockFile);
    }

    anomalies
}

/// The `.unblock.lock` path next to `db` (in the db's parent directory).
fn lock_file_path(db: &Path) -> PathBuf {
    db.parent().map_or_else(
        || db.with_file_name(LOCK_FILE_NAME),
        |parent| parent.join(LOCK_FILE_NAME),
    )
}

/// Whether the db opens but its first 16 bytes are not the `SQLite` magic (or are shorter than 16).
///
/// An OPEN failure returns `false` (not flagged) — faithful to the original's conservative
/// `if let Ok(file) = File::open(..)` guard.
fn db_header_mismatches_sqlite(db: &Path) -> bool {
    use std::io::Read as _;

    let Ok(mut file) = std::fs::File::open(db) else {
        return false;
    };
    let mut header = [0_u8; 16];
    file.read_exact(&mut header).is_err() || header != SQLITE_MAGIC
}

/// Whether the `-wal` at `wal_path` is a NON-EMPTY sidecar smaller than its 32-byte header.
///
/// A metadata failure is not treated as truncation (conservative). This refines the original's bare
/// `len < 32`: an **empty (0-byte)** `-wal` is a **valid** state `SQLite` leaves for a live-open or
/// freshly-checkpointed connection, so it is NOT flagged. In beads the offline doctor closed its
/// connection (checkpointing the WAL away) before classifying, so a healthy workspace never showed
/// this anomaly; unblock's persistent-open `Session` classifies the DB it currently holds open, whose
/// transient WAL is empty — treating `0 < len < 32` as truncated preserves beads' OBSERVABLE behavior
/// (healthy → clean) while still catching a genuinely truncated header (1–31 bytes).
fn wal_is_truncated(wal_path: &Path) -> bool {
    std::fs::metadata(wal_path).is_ok_and(|meta| (1..MIN_WAL_LEN).contains(&meta.len()))
}

/// Whether any line of `path` starts with a git conflict marker (`<<<<<<< `, `>>>>>>> `, `======= `,
/// or `||||||| ` — the four 7-byte diff3 sigils).
///
/// Reads the file as **raw bytes in a single O(n) pass** (a `BufReader`), so non-UTF-8 content cannot
/// hide markers; any open/read failure conservatively reports `false` (absence of evidence, not
/// evidence of corruption). Faithful byte-for-byte port of the original scanner.
#[must_use]
pub fn jsonl_has_conflict_markers(path: &Path) -> bool {
    use std::io::BufRead as _;

    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut reader = std::io::BufReader::with_capacity(64 * 1024, file);
    let mut prefix = [0_u8; CONFLICT_MARKER_PREFIX_LEN];
    let mut prefix_len = 0_usize;
    let mut reading_prefix = true;

    loop {
        let buffer = match reader.fill_buf() {
            Ok([]) | Err(_) => return false,
            Ok(buffer) => buffer,
        };

        let mut consumed = 0;
        for &byte in buffer {
            consumed += 1;

            if reading_prefix && byte != b'\n' {
                if let Some(slot) = prefix.get_mut(prefix_len) {
                    *slot = byte;
                    prefix_len += 1;
                }
                if prefix_len == CONFLICT_MARKER_PREFIX_LEN {
                    if is_conflict_marker_prefix(prefix) {
                        return true;
                    }
                    reading_prefix = false;
                }
            }

            if byte == b'\n' {
                prefix_len = 0;
                reading_prefix = true;
            }
        }

        reader.consume(consumed);
    }
}

/// Whether the 7-byte `prefix` is one of the four git conflict-marker sigils.
fn is_conflict_marker_prefix(prefix: [u8; CONFLICT_MARKER_PREFIX_LEN]) -> bool {
    prefix == *b"<<<<<<<" || prefix == *b">>>>>>>" || prefix == *b"=======" || prefix == *b"|||||||"
}

/// Whether the lock file at `lock_path` is orphaned: its `mtime` is at least
/// [`ORPHANED_LOCK_FILE_STALE_AFTER`] before `now`. A future `mtime` (clock skew) is NOT stale, and a
/// metadata/mtime failure is NOT stale (conservative). `now` is injectable for deterministic tests.
#[must_use]
pub fn is_orphaned_lock_file(lock_path: &Path, now: SystemTime) -> bool {
    std::fs::metadata(lock_path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .is_some_and(|modified| lock_modified_time_is_stale(modified, now))
}

/// Whether `modified` is at least [`ORPHANED_LOCK_FILE_STALE_AFTER`] before `now` (a future
/// `modified` is not stale).
fn lock_modified_time_is_stale(modified: SystemTime, now: SystemTime) -> bool {
    matches!(
        now.duration_since(modified),
        Ok(age) if age >= ORPHANED_LOCK_FILE_STALE_AFTER
    )
}

#[cfg(test)]
mod tests {
    use super::{
        FileAnomaly, ORPHANED_LOCK_FILE_STALE_AFTER, classify_file_state,
        is_conflict_marker_prefix, jsonl_has_conflict_markers, lock_modified_time_is_stale,
    };
    use crate::level::HealthLevel;
    use std::io::Write as _;
    use std::time::{Duration, SystemTime};
    use tempfile::TempDir;

    fn write_sqlite_db(path: &std::path::Path) {
        let mut f = std::fs::File::create(path).expect("create db");
        f.write_all(b"SQLite format 3\0").expect("magic");
        f.write_all(&[0_u8; 100]).expect("body");
    }

    #[test]
    fn each_variant_has_a_stable_code() {
        assert_eq!(FileAnomaly::DatabaseMissing.code(), "database_missing");
        assert_eq!(FileAnomaly::DatabaseNotSqlite.code(), "database_not_sqlite");
        assert_eq!(
            FileAnomaly::SidecarMismatch {
                has_wal: false,
                has_shm: true,
            }
            .code(),
            "sidecar_mismatch"
        );
        assert_eq!(FileAnomaly::TruncatedWal.code(), "truncated_wal");
        assert_eq!(
            FileAnomaly::JournalSidecarPresent.code(),
            "journal_sidecar_present"
        );
        assert_eq!(FileAnomaly::OrphanedLockFile.code(), "orphaned_lock_file");
        assert_eq!(
            FileAnomaly::JsonlConflictMarkers.code(),
            "jsonl_conflict_markers"
        );
    }

    #[test]
    fn severity_map_matches_d29_f5() {
        // Only conflict markers are Unsafe; every other v1-lite anomaly is Recoverable.
        assert_eq!(
            FileAnomaly::JsonlConflictMarkers.severity(),
            HealthLevel::Unsafe
        );
        for anomaly in [
            FileAnomaly::DatabaseMissing,
            FileAnomaly::DatabaseNotSqlite,
            FileAnomaly::SidecarMismatch {
                has_wal: false,
                has_shm: true,
            },
            FileAnomaly::TruncatedWal,
            FileAnomaly::JournalSidecarPresent,
            FileAnomaly::OrphanedLockFile,
        ] {
            assert_eq!(anomaly.severity(), HealthLevel::Recoverable, "{anomaly:?}");
        }
    }

    #[test]
    fn healthy_workspace_has_no_anomalies() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("unblock.db");
        let jsonl = dir.path().join("issues.jsonl");
        write_sqlite_db(&db);
        std::fs::write(&jsonl, "{\"id\":\"ub-1\"}\n").unwrap();
        assert!(classify_file_state(&db, Some(&jsonl)).is_empty());
    }

    #[test]
    fn lone_wal_without_shm_is_not_a_mismatch() {
        // A lone `-wal` is normal for libsql; only `-shm` WITHOUT `-wal` is a mismatch.
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("unblock.db");
        write_sqlite_db(&db);
        std::fs::write(crate::paths::sidecar(&db, "-wal"), [0_u8; 64]).unwrap();
        let anomalies = classify_file_state(&db, None);
        assert!(
            !anomalies
                .iter()
                .any(|a| matches!(a, FileAnomaly::SidecarMismatch { .. })),
            "{anomalies:?}"
        );
    }

    #[test]
    fn is_conflict_marker_prefix_covers_all_four_sigils() {
        assert!(is_conflict_marker_prefix(*b"<<<<<<<"));
        assert!(is_conflict_marker_prefix(*b">>>>>>>"));
        assert!(is_conflict_marker_prefix(*b"======="));
        assert!(is_conflict_marker_prefix(*b"|||||||"));
        assert!(!is_conflict_marker_prefix(*b"{\"id\":\""));
    }

    #[test]
    fn conflict_scan_is_false_on_a_missing_file() {
        let dir = TempDir::new().unwrap();
        assert!(!jsonl_has_conflict_markers(
            &dir.path().join("absent.jsonl")
        ));
    }

    #[test]
    fn conflict_scan_detects_a_marker_hidden_after_non_utf8_bytes() {
        let dir = TempDir::new().unwrap();
        let jsonl = dir.path().join("issues.jsonl");
        std::fs::write(&jsonl, b"{\"id\":\"a\"}\n\xff\n<<<<<<< HEAD\n").unwrap();
        assert!(jsonl_has_conflict_markers(&jsonl));
    }

    #[test]
    fn stale_and_fresh_and_future_lock_mtimes() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_hours(1);
        let stale = now
            .checked_sub(ORPHANED_LOCK_FILE_STALE_AFTER + Duration::from_secs(1))
            .unwrap();
        let fresh = now
            .checked_sub(ORPHANED_LOCK_FILE_STALE_AFTER.saturating_sub(Duration::from_secs(1)))
            .unwrap();
        let future = now + Duration::from_secs(1);
        assert!(lock_modified_time_is_stale(stale, now));
        assert!(!lock_modified_time_is_stale(fresh, now));
        assert!(!lock_modified_time_is_stale(future, now));
    }
}
