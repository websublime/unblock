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
//! **Lock safety (D29 clause F6, v1.0.3).** [`classify_file_state`] and `run_doctor` never close a
//! descriptor on the database file or a sidecar while a session is open, and open none on a file
//! that already stats as one: the 16-byte header is a PRE-OPEN fact ([`DbHeaderProbe::read`], called
//! before any connection exists) that the classifier takes as data, and its JSONL conflict scan
//! skips (and never closes) a JSONL that is one of the session's files (`unblock_sync::SessionFiles`).
//! Closing ANY descriptor on a file releases every POSIX lock the process holds on it (sqlite.org
//! `howtocorrupt.html` §2.2). The two public byte readers are NOT session-safe on their own:
//! [`DbHeaderProbe::read`] opens and closes the database (call it only before any connection opens),
//! and [`jsonl_has_conflict_markers`] opens and closes its path (never call it inside a live session;
//! the doctor uses the guarded scan).
//!
//! [`Recoverable`]: HealthLevel::Recoverable
//! [`Unsafe`]: HealthLevel::Unsafe
//! [`JsonlConflictMarkers`]: FileAnomaly::JsonlConflictMarkers

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use schemars::JsonSchema;
use serde::Serialize;
use unblock_sync::SessionFiles;

use crate::error::HealthError;
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

/// What the first 16 bytes of the database file were BEFORE this process opened any `SQLite`
/// connection to it (PRD §4 D29 clause F6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbHeader {
    /// No file at the path (`NotFound`); libsql creates it on open.
    Absent,
    /// A zero-length file; `SQLite` treats it as an empty database (not a mismatch).
    Empty,
    /// The first 16 bytes are `"SQLite format 3\0"`.
    Sqlite,
    /// Non-empty, and shorter than 16 bytes or not the `SQLite` magic; or a regular file that opened
    /// but could not be read.
    NotSqlite,
    /// The path exists but is not a regular file (never opened: a FIFO would block the open), or a
    /// regular file that could not be opened (permission) or `fstat`ed.
    Unreadable,
    /// The file at the path after the open is not the one the probe read (a different
    /// `(dev, ino)`, or the post-open stat failed). No header verdict: NOT a mismatch. Unix only: off
    /// unix the probe records no identity and the pre-open header is returned.
    Superseded,
}

impl DbHeader {
    /// Whether this header is a `DatabaseNotSqlite` finding: `true` ONLY for [`DbHeader::NotSqlite`].
    #[must_use]
    pub fn mismatches_sqlite(self) -> bool {
        matches!(self, Self::NotSqlite)
    }
}

/// `(dev, ino)` of the descriptor the probe read.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileId {
    dev: u64,
    ino: u64,
}

/// A pre-open header read of one path. Built only by [`DbHeaderProbe::read`]; not `Clone`.
#[derive(Debug, PartialEq, Eq)]
pub struct DbHeaderProbe {
    path: PathBuf,
    header: DbHeader,
    /// `None` when no regular file was read.
    #[cfg(unix)]
    identity: Option<FileId>,
}

impl DbHeaderProbe {
    /// Read `db`'s header. MUST run before this process opens any `SQLite` connection to `db`:
    /// closing the descriptor drops every POSIX lock the process holds on the file (D29 clause F6).
    /// NOT session-safe: it opens and closes `db`; call it only pre-open.
    ///
    /// Stats first and opens ONLY a regular file (a FIFO is never opened). Sync `std::fs` I/O (one
    /// stat, at most one 16-byte read) on the cli's async path: the same posture as v1.0.2, where
    /// this read ran inline in async `Session::doctor`, and as config resolution's `read_to_string`;
    /// `spawn_blocking` is not used because the work is bounded and runs before any connection
    /// exists.
    #[must_use]
    pub fn read(db: &Path) -> Self {
        let probed = read_header(db);
        Self {
            path: db.to_path_buf(),
            header: probed.header,
            #[cfg(unix)]
            identity: probed.identity,
        }
    }

    /// The path the probe read.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The pre-open header (no post-open identity check; for fixtures and diagnostics).
    #[must_use]
    pub fn header(&self) -> DbHeader {
        self.header
    }

    /// Consume the probe for `db`, AFTER the open: `Err` if the probe read another path; else, on
    /// unix, stat `db` (no descriptor) and return [`DbHeader::Superseded`] when the probe recorded a
    /// file identity and the stat fails or yields another `(dev, ino)`; else the pre-open header.
    ///
    /// # Errors
    ///
    /// [`HealthError::DbHeaderProbePathMismatch`] when the probe read another path.
    pub fn header_for(self, db: &Path) -> Result<DbHeader, HealthError> {
        self.header_and_session(db).map(|(header, _)| header)
    }

    /// The ONE consuming call `run_doctor` makes, AFTER the open: the post-open header (the
    /// [`header_for`](Self::header_for) rules) AND the session's files —
    /// `SessionFiles::of_database(db)` plus, on unix, the probe's own inode, so a JSONL that is a hard
    /// link to the inode the connection opened is recognized even after the db's NAME was renamed
    /// away. Off unix the header is the pre-open header and the set is `of_database(db)` alone.
    ///
    /// # Errors
    ///
    /// [`HealthError::DbHeaderProbePathMismatch`] when the probe read another path.
    pub(crate) fn header_and_session(
        self,
        db: &Path,
    ) -> Result<(DbHeader, SessionFiles), HealthError> {
        if self.path != db {
            return Err(HealthError::DbHeaderProbePathMismatch {
                probed: self.path,
                expected: db.to_path_buf(),
            });
        }
        let session = SessionFiles::of_database(db);
        #[cfg(unix)]
        if let Some(probed) = self.identity {
            let header = if stat_id(db) == Some(probed) {
                self.header
            } else {
                DbHeader::Superseded
            };
            return Ok((header, session.with_inode(probed.dev, probed.ino)));
        }
        Ok((self.header, session))
    }
}

/// `(dev, ino)` of `p` by `stat` (no descriptor).
#[cfg(unix)]
fn stat_id(p: &Path) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(p).ok().map(|m| FileId {
        dev: m.dev(),
        ino: m.ino(),
    })
}

/// The result of [`read_header`].
struct Probed {
    header: DbHeader,
    #[cfg(unix)]
    identity: Option<FileId>,
}

impl Probed {
    /// A verdict reached without reading a regular file (no identity recorded).
    fn without_file(header: DbHeader) -> Self {
        Self {
            header,
            #[cfg(unix)]
            identity: None,
        }
    }
}

/// The pre-open header read: stat → (regular file only) open → fstat → bounded read → verdict.
fn read_header(db: &Path) -> Probed {
    use std::io::Read as _;

    // (1) stat first: a missing path is `Absent`; any other error is `Unreadable`.
    let meta = match std::fs::metadata(db) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Probed::without_file(DbHeader::Absent);
        }
        Err(_) => return Probed::without_file(DbHeader::Unreadable),
    };
    // (2) not a regular file (directory, FIFO, socket, device): never opened.
    if !meta.is_file() {
        return Probed::without_file(DbHeader::Unreadable);
    }
    // (3) open: any error is `Unreadable` (the conservative F5 "cannot be opened" rule).
    let Ok(mut file) = std::fs::File::open(db) else {
        return Probed::without_file(DbHeader::Unreadable);
    };
    // (4) fstat the opened descriptor: a failure, or a non-regular file (the path was swapped between
    //     the stat and the open), is `Unreadable`; else its identity is recorded.
    let Ok(opened) = file.metadata() else {
        return Probed::without_file(DbHeader::Unreadable);
    };
    if !opened.is_file() {
        return Probed::without_file(DbHeader::Unreadable);
    }
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt as _;
        Some(FileId {
            dev: opened.dev(),
            ino: opened.ino(),
        })
    };
    // (5) read up to 16 bytes, retrying `Interrupted`; any other read error → `None` (→ `NotSqlite`).
    let mut buf = [0_u8; 16];
    let mut filled = 0_usize;
    let read_ok = loop {
        let Some(rest) = buf.get_mut(filled..) else {
            break true;
        };
        if rest.is_empty() {
            break true;
        }
        match file.read(rest) {
            Ok(0) => break true,
            Ok(n) => filled += n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break false,
        }
    };
    // The `File` drops here, before any connection exists, so this close is harmless.
    drop(file);
    let bytes = if read_ok { buf.get(..filled) } else { None };
    Probed {
        header: verdict(bytes),
        #[cfg(unix)]
        identity,
    }
}

/// Steps 5–6 of the probe: `None` (the read failed) → `NotSqlite` (parity with v1.0.2); `0` bytes →
/// `Empty`; the 16-byte magic → `Sqlite`; anything else (1–15 bytes, or a wrong magic) → `NotSqlite`.
fn verdict(bytes: Option<&[u8]>) -> DbHeader {
    match bytes {
        Some([]) => DbHeader::Empty,
        Some(b) if b == SQLITE_MAGIC.as_slice() => DbHeader::Sqlite,
        None | Some(_) => DbHeader::NotSqlite,
    }
}

/// Classify the on-disk state of a workspace into a deterministic [`Vec`] of [`FileAnomaly`].
///
/// **Pure** — inspects the filesystem only (no DB handle, no writes). `jsonl` is `None` when JSONL
/// export is not configured (then no JSONL-derived anomaly can fire). Findings are pushed in
/// variant-declaration order (fixed for snapshot stability, NFR-14). Firing conditions are a faithful
/// port of the original:
/// - [`DatabaseMissing`](FileAnomaly::DatabaseMissing): the db is NOT a file AND the jsonl IS a file.
/// - [`DatabaseNotSqlite`](FileAnomaly::DatabaseNotSqlite): the db is a file and the PRE-OPEN
///   `db_header` [mismatches](DbHeader::mismatches_sqlite) (D29 clause F6: this function reads no
///   header itself; a 0-byte, unopenable or swapped file is not flagged).
/// - [`SidecarMismatch`](FileAnomaly::SidecarMismatch): `has_shm && !has_wal` only (a lone `-wal` is
///   normal for libsql).
/// - [`TruncatedWal`](FileAnomaly::TruncatedWal): `has_wal` and the `-wal` file is non-empty but
///   `< 32` bytes (an empty 0-byte WAL is valid — see [`wal_is_truncated`]).
/// - [`JsonlConflictMarkers`](FileAnomaly::JsonlConflictMarkers): the jsonl is a file and contains a
///   git conflict marker (see [`jsonl_has_conflict_markers`]); a JSONL that is the database or one of
///   its sidecars is never scanned and never closed (D29 clause F6).
/// - [`JournalSidecarPresent`](FileAnomaly::JournalSidecarPresent): a `-journal` sidecar exists.
/// - [`OrphanedLockFile`](FileAnomaly::OrphanedLockFile): a `.unblock.lock` exists next to the db and
///   is stale (see [`is_orphaned_lock_file`]).
#[must_use]
pub fn classify_file_state(
    db: &Path,
    db_header: DbHeader,
    jsonl: Option<&Path>,
) -> Vec<FileAnomaly> {
    classify_file_state_in(db, db_header, jsonl, &SessionFiles::of_database(db))
}

/// [`classify_file_state`] with an explicit session-file set (`run_doctor` adds the probe's inode).
pub(crate) fn classify_file_state_in(
    db: &Path,
    db_header: DbHeader,
    jsonl: Option<&Path>,
    session: &SessionFiles,
) -> Vec<FileAnomaly> {
    let mut anomalies = Vec::new();

    let db_is_file = db.is_file();

    // DatabaseMissing: the db is gone but a JSONL export survives to recover from.
    if !db_is_file && jsonl.is_some_and(Path::is_file) {
        anomalies.push(FileAnomaly::DatabaseMissing);
    }

    // DatabaseNotSqlite: the db is present but its header is not the SQLite magic.
    if db_is_file && db_header.mismatches_sqlite() {
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
    if jsonl.is_some_and(|p| p.is_file() && jsonl_has_conflict_markers_in(p, session)) {
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
///
/// NOT session-safe: it opens and closes `path`; never call it on a path that may be the live
/// database or a sidecar inside a session (the doctor classifier uses the guarded scan, D29 clause F6).
#[must_use]
pub fn jsonl_has_conflict_markers(path: &Path) -> bool {
    std::fs::File::open(path).is_ok_and(file_has_conflict_markers)
}

/// [`jsonl_has_conflict_markers`] for the doctor classifier: a JSONL that is one of the session's
/// files is never scanned (D29 clause F6).
fn jsonl_has_conflict_markers_in(path: &Path, session: &SessionFiles) -> bool {
    match open_unless_session_file(session, path) {
        Ok(Some(file)) => file_has_conflict_markers(file),
        Ok(None) | Err(_) => false,
    }
}

/// Open the JSONL for the conflict scan unless it is one of the session's files: (1)
/// `session.contains(p)` → `Ok(None)`, never opened; (2) `session.open_verified(p)` — open, `fstat`
/// the opened descriptor, and on a session file keep it open forever and return `Ok(None)`.
/// `unblock doctor` is short-lived, so at most one descriptor is held per racing run until the
/// process exits.
fn open_unless_session_file(
    session: &SessionFiles,
    p: &Path,
) -> std::io::Result<Option<std::fs::File>> {
    if session.contains(p) {
        return Ok(None);
    }
    session.open_verified(p)
}

/// The single O(n) raw-byte marker scan over an opened file (the faithful port of the original).
fn file_has_conflict_markers(file: std::fs::File) -> bool {
    use std::io::BufRead as _;

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
        DbHeader, DbHeaderProbe, FileAnomaly, ORPHANED_LOCK_FILE_STALE_AFTER, classify_file_state,
        classify_file_state_in, is_conflict_marker_prefix, jsonl_has_conflict_markers,
        lock_modified_time_is_stale, verdict,
    };
    use crate::level::HealthLevel;
    use std::io::Write as _;
    use std::time::{Duration, SystemTime};
    use tempfile::TempDir;
    use unblock_sync::SessionFiles;

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
        assert!(
            classify_file_state(&db, DbHeaderProbe::read(&db).header(), Some(&jsonl)).is_empty()
        );
    }

    #[test]
    fn lone_wal_without_shm_is_not_a_mismatch() {
        // A lone `-wal` is normal for libsql; only `-shm` WITHOUT `-wal` is a mismatch.
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("unblock.db");
        write_sqlite_db(&db);
        std::fs::write(crate::paths::sidecar(&db, "-wal"), [0_u8; 64]).unwrap();
        let anomalies = classify_file_state(&db, DbHeaderProbe::read(&db).header(), None);
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

    // ---- D29 clause F6 (v1.0.3, `ub-kpe`): the pre-open header probe and the session-file guard ----

    const MARKED: &[u8] = b"SQLite format 3\0junk\n<<<<<<< HEAD\nmore\n";

    fn has_marker_finding(anomalies: &[FileAnomaly]) -> bool {
        anomalies.contains(&FileAnomaly::JsonlConflictMarkers)
    }

    #[test]
    fn db_header_probe_classifies_every_file_state() {
        let dir = TempDir::new().unwrap();
        let p = |name: &str| dir.path().join(name);
        assert_eq!(
            DbHeaderProbe::read(&p("absent.db")).header(),
            DbHeader::Absent
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(p("nowhere"), p("dangling.db")).unwrap();
            assert_eq!(
                DbHeaderProbe::read(&p("dangling.db")).header(),
                DbHeader::Absent
            );
        }
        std::fs::write(p("empty.db"), b"").unwrap();
        assert_eq!(
            DbHeaderProbe::read(&p("empty.db")).header(),
            DbHeader::Empty
        );
        write_sqlite_db(&p("ok.db"));
        assert_eq!(DbHeaderProbe::read(&p("ok.db")).header(), DbHeader::Sqlite);
        std::fs::write(p("short.db"), b"short").unwrap();
        assert_eq!(
            DbHeaderProbe::read(&p("short.db")).header(),
            DbHeader::NotSqlite
        );
        std::fs::write(p("text.db"), [b'x'; 64]).unwrap();
        assert_eq!(
            DbHeaderProbe::read(&p("text.db")).header(),
            DbHeader::NotSqlite
        );
        std::fs::create_dir(p("dir.db")).unwrap();
        assert_eq!(
            DbHeaderProbe::read(&p("dir.db")).header(),
            DbHeader::Unreadable
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::write(p("locked.db"), [b'x'; 64]).unwrap();
            std::fs::set_permissions(p("locked.db"), std::fs::Permissions::from_mode(0o000))
                .unwrap();
            // Root opens a mode-000 file anyway; only assert when the open really fails.
            if std::fs::File::open(p("locked.db")).is_err() {
                assert_eq!(
                    DbHeaderProbe::read(&p("locked.db")).header(),
                    DbHeader::Unreadable
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn db_header_probe_never_opens_a_fifo() {
        let dir = TempDir::new().unwrap();
        let fifo = dir.path().join("unblock.db");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("run mkfifo");
        assert!(status.success(), "mkfifo failed");
        let (tx, rx) = std::sync::mpsc::channel();
        let path = fifo.clone();
        std::thread::spawn(move || {
            let _ = tx.send(DbHeaderProbe::read(&path).header());
        });
        let header = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the probe opened the FIFO and blocked");
        assert_eq!(header, DbHeader::Unreadable);
    }

    #[test]
    fn read_error_after_open_is_not_sqlite() {
        assert_eq!(verdict(None), DbHeader::NotSqlite);
        assert_eq!(verdict(Some(&[])), DbHeader::Empty);
        assert_eq!(verdict(Some(b"SQLite format 3\0")), DbHeader::Sqlite);
        assert_eq!(verdict(Some(b"SQLite format 3")), DbHeader::NotSqlite);
    }

    #[cfg(unix)]
    #[test]
    fn header_for_detects_a_swapped_file() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("unblock.db");
        // A non-SQLite file probed, then a valid db renamed over it → Superseded, no finding.
        std::fs::write(&db, [b'x'; 64]).unwrap();
        let probe = DbHeaderProbe::read(&db);
        assert_eq!(probe.header(), DbHeader::NotSqlite);
        let valid = dir.path().join("valid.db");
        write_sqlite_db(&valid);
        std::fs::rename(&valid, &db).unwrap();
        let header = probe.header_for(&db).unwrap();
        assert_eq!(header, DbHeader::Superseded);
        assert!(!classify_file_state(&db, header, None).contains(&FileAnomaly::DatabaseNotSqlite));
        // Control: an `Empty` probe, then the magic written IN PLACE (same inode) → Empty.
        let inplace = dir.path().join("inplace.db");
        std::fs::write(&inplace, b"").unwrap();
        let probe = DbHeaderProbe::read(&inplace);
        write_sqlite_db(&inplace);
        assert_eq!(probe.header_for(&inplace).unwrap(), DbHeader::Empty);
        // Control: an `Absent` probe, then the file created → Absent.
        let created = dir.path().join("created.db");
        let probe = DbHeaderProbe::read(&created);
        write_sqlite_db(&created);
        assert_eq!(probe.header_for(&created).unwrap(), DbHeader::Absent);
        // A probe whose file is deleted before `header_for` → Superseded.
        let gone = dir.path().join("gone.db");
        write_sqlite_db(&gone);
        let probe = DbHeaderProbe::read(&gone);
        std::fs::remove_file(&gone).unwrap();
        assert_eq!(probe.header_for(&gone).unwrap(), DbHeader::Superseded);
    }

    #[cfg(unix)]
    #[test]
    fn the_probed_inode_stays_a_session_file_after_a_rename() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("unblock.db");
        std::fs::write(&db, MARKED).unwrap();
        let probe = DbHeaderProbe::read(&db);
        // `alias.jsonl` → A; then B renamed over the db path: A survives only as `alias.jsonl`.
        let alias = dir.path().join("alias.jsonl");
        std::fs::hard_link(&db, &alias).unwrap();
        let b = dir.path().join("b.db");
        write_sqlite_db(&b);
        std::fs::rename(&b, &db).unwrap();
        let (header, session) = probe.header_and_session(&db).unwrap();
        assert_eq!(header, DbHeader::Superseded);
        assert!(
            session.contains(&alias),
            "the probed inode must stay in the set"
        );
        assert!(!has_marker_finding(&classify_file_state_in(
            &db,
            header,
            Some(&alias),
            &session
        )));
        // Control: a DISTINCT file with the same bytes is flagged.
        let distinct = dir.path().join("distinct.jsonl");
        std::fs::write(&distinct, MARKED).unwrap();
        assert!(has_marker_finding(&classify_file_state_in(
            &db,
            header,
            Some(&distinct),
            &session
        )));
    }

    #[test]
    fn conflict_scan_skips_the_db_itself() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("unblock.db");
        std::fs::write(&db, MARKED).unwrap();
        let header = DbHeaderProbe::read(&db).header();
        // Same path → no conflict-marker finding.
        assert!(!has_marker_finding(&classify_file_state(
            &db,
            header,
            Some(&db)
        )));
        #[cfg(unix)]
        {
            let link = dir.path().join("sym.jsonl");
            std::os::unix::fs::symlink(&db, &link).unwrap();
            assert!(!has_marker_finding(&classify_file_state(
                &db,
                header,
                Some(&link)
            )));
            let hard = dir.path().join("hard.jsonl");
            std::fs::hard_link(&db, &hard).unwrap();
            assert!(!has_marker_finding(&classify_file_state(
                &db,
                header,
                Some(&hard)
            )));
            for suffix in ["-wal", "-shm", "-journal"] {
                let sidecar = crate::paths::sidecar(&db, suffix);
                std::fs::write(&sidecar, MARKED).unwrap();
                assert!(
                    !has_marker_finding(&classify_file_state(&db, header, Some(&sidecar))),
                    "{suffix} by its own name"
                );
                let alias = dir.path().join(format!("alias{suffix}.jsonl"));
                std::fs::hard_link(&sidecar, &alias).unwrap();
                assert!(
                    !has_marker_finding(&classify_file_state(&db, header, Some(&alias))),
                    "{suffix} by a hard link"
                );
                std::fs::remove_file(&sidecar).unwrap();
            }
            // The race branch: the post-open check on the opened descriptor.
            assert!(
                SessionFiles::of_database(&db)
                    .open_verified(&hard)
                    .unwrap()
                    .is_none()
            );
        }
        // Control: a DISTINCT file with the same bytes is flagged.
        let distinct = dir.path().join("distinct.jsonl");
        std::fs::write(&distinct, MARKED).unwrap();
        assert!(has_marker_finding(&classify_file_state(
            &db,
            header,
            Some(&distinct)
        )));
    }

    /// A symlinked database: `SQLite` keeps the sidecars next to the TARGET, so a JSONL hard-linked to
    /// the target's `-shm` is a session file too (scan skipped, never opened-and-closed).
    #[cfg(unix)]
    #[test]
    fn conflict_scan_skips_the_target_sidecars_of_a_symlinked_db() {
        let dir = TempDir::new().unwrap();
        let store = dir.path().join("store");
        std::fs::create_dir(&store).unwrap();
        let real = store.join("unblock.db");
        std::fs::write(&real, MARKED).unwrap();
        let shm = crate::paths::sidecar(&real, "-shm");
        std::fs::write(&shm, MARKED).unwrap();
        let db = dir.path().join("unblock.db");
        std::os::unix::fs::symlink(&real, &db).unwrap();
        let alias = dir.path().join("alias.jsonl");
        std::fs::hard_link(&shm, &alias).unwrap();
        let header = DbHeaderProbe::read(&db).header();
        assert!(!has_marker_finding(&classify_file_state(
            &db,
            header,
            Some(&alias)
        )));
        let probe = DbHeaderProbe::read(&db);
        let (header, session) = probe.header_and_session(&db).unwrap();
        assert!(session.contains(&alias));
        assert!(!has_marker_finding(&classify_file_state_in(
            &db,
            header,
            Some(&alias),
            &session
        )));
    }

    #[test]
    fn only_not_sqlite_mismatches() {
        assert!(DbHeader::NotSqlite.mismatches_sqlite());
        for header in [
            DbHeader::Absent,
            DbHeader::Empty,
            DbHeader::Sqlite,
            DbHeader::Unreadable,
            DbHeader::Superseded,
        ] {
            assert!(!header.mismatches_sqlite(), "{header:?}");
        }
    }

    #[test]
    fn classify_trusts_the_preopen_header_not_the_file() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("unblock.db");
        std::fs::write(&db, [b'x'; 64]).unwrap();
        assert!(
            !classify_file_state(&db, DbHeader::Sqlite, None)
                .contains(&FileAnomaly::DatabaseNotSqlite)
        );
        write_sqlite_db(&db);
        assert!(
            classify_file_state(&db, DbHeader::NotSqlite, None)
                .contains(&FileAnomaly::DatabaseNotSqlite)
        );
        assert!(
            !classify_file_state(&db, DbHeader::Empty, None)
                .contains(&FileAnomaly::DatabaseNotSqlite)
        );
    }

    #[test]
    fn database_missing_ignores_the_header() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("unblock.db");
        let jsonl = dir.path().join("issues.jsonl");
        std::fs::write(&jsonl, "{\"id\":\"ub-1\"}\n").unwrap();
        for header in [
            DbHeader::Absent,
            DbHeader::Empty,
            DbHeader::Sqlite,
            DbHeader::NotSqlite,
            DbHeader::Unreadable,
            DbHeader::Superseded,
        ] {
            assert_eq!(
                classify_file_state(&db, header, Some(&jsonl)),
                vec![FileAnomaly::DatabaseMissing],
                "{header:?}"
            );
        }
    }
}
