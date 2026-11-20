//! The LOCAL-file health primitives (PRD §4 D58 clauses (2), (5), (6) and (7); spine §3.3).
//!
//! They take paths and return plain storage-owned types. They sit outside the
//! [`Storage`](crate::Storage) trait because remote mode has no file, and their file I/O failures
//! surface as [`StorageError::RecoveryIo`].
//!
//! - [`lost_database`] stats the database file and `.write.lock`, and opens neither.
//! - [`live_connections_to`] counts this process's open libsql connections to one database file.
//! - [`probe_write_lock`] makes one non-blocking `try_lock` on an existing `.write.lock`.
//! - [`exclusive_window`] proves that no other connection has the database open, copies the raw
//!   files as evidence, and runs the pre-open repairs.
//!
//! # The exclusive window
//!
//! A refusal at any step skips every later step and comes back as `Ok` with
//! [`ExclusiveOutcome::refused`] set.
//!
//! 1. **(a)** The database file must exist. `.write.lock` gets one `try_lock` and is created when
//!    absent.
//! 2. **(a2)** No libsql connection in this process may be open on the database file. Closing any
//!    `std::fs` descriptor on that file cancels every POSIX lock the process holds on it
//!    (sqlite.org `howtocorrupt.html` section 2.2), and step (b) opens and closes one. The run
//!    directory must then be a real directory under a real `<db dir>/.recovery/`, confined by
//!    `lstat` and identity, never by resolving both sides.
//! 3. **(b)** The database and each present `-wal`, `-shm` and `-journal` must be a regular file
//!    per `lstat`, checked before `raw/` exists, else the window refuses `not_a_regular_file`.
//!    They are then copied into `<run_dir>/raw/` on a blocking thread, and each live file's
//!    metadata is recorded first.
//! 4. **(c)** One connection with `busy_timeout = 0` runs `PRAGMA locking_mode=EXCLUSIVE` as its
//!    first statement, then `BEGIN EXCLUSIVE` and `COMMIT`. In WAL mode every other connection keeps
//!    a SHARED lock on the database file while it is open, so the exclusive lock fails with
//!    `SQLITE_BUSY` [INFERENCE, measured by `tests/exclusive_window.rs`]. Any other failure refuses
//!    `unexpected_open_result` with its raw result code. The evidence check then compares the live
//!    files against step (b).
//! 5. **(d)** `VACUUM INTO <run_dir>/snapshot.db` runs, then `PRAGMA wal_checkpoint(TRUNCATE)` and
//!    the `-shm` rename when requested. The exclusive connection never maps the `-shm`.
//! 6. **(e)** The connection closes, and `SQLite` checkpoints and deletes the `-wal` itself. A hot
//!    `-journal` that step (c) rolled back is deleted by `SQLite` at the same close (PRD §4 D58
//!    amendment, measured 2026-11-13). That rollback IS the `quarantine_journal` repair: its
//!    evidence is the pre-rollback copy under `raw/`, and nothing is renamed. The lock is released
//!    last.
//!
//! On unix `raw/` and `quarantine/` are created 0700, and the raw copies, the renamed files and
//! `snapshot.db` are left 0600. The window never unlinks a file and never renames a `-wal` that
//! `SQLite` holds open. A database `SQLite` rejects is Unsafe (PRD §4 D58, tracker comment 389),
//! so the window never moves the database file.

use std::fmt;
use std::fs::{self, File, Metadata, OpenOptions, TryLockError};
use std::io::{self, ErrorKind, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use libsql::ffi::SQLITE_BUSY;
use libsql::{Builder, Connection, OpenFlags};

use super::close_once::{CloseOnceConnection, FileKey, registered_connections};
use super::lock::WRITE_LOCK_FILE_NAME;
use super::recovery_fs::{confine, create_private_dir, real_dir, restrict_file};
use crate::error::{StorageError, map_libsql_err};

/// The run-directory subdirectory that holds the step (b) copies.
const RAW_DIR_NAME: &str = "raw";
/// The run-directory subdirectory that receives every renamed file.
const QUARANTINE_DIR_NAME: &str = "quarantine";
/// The step (d) snapshot inside the run directory.
const SNAPSHOT_FILE_NAME: &str = "snapshot.db";
/// The read size of the byte comparisons.
const CHUNK_LEN: usize = 64 * 1024;

/// `SQLite`'s lock-byte range, the 512 bytes at offset `0x4000_0000` that never hold data. Windows'
/// mandatory locks make it unreadable while step (c) holds the database.
#[cfg(windows)]
const LOCK_BYTE_RANGE: (u64, u64) = (0x4000_0000, 0x4000_0200);

/// A testkit hook the window calls with the database path. It runs between steps (b) and (c), or at
/// one of the two observation points inside step (c).
#[cfg(feature = "testkit")]
type WindowHook = Box<dyn Fn(&Path) + Send + Sync>;

/// The pre-open repairs one [`exclusive_window`] runs (spine §3.1).
///
/// The fields are private, so the type is the same with storage's `testkit` feature on or off.
/// [`ExclusiveRequest::new`] turns every action off, and one setter turns each action on.
pub struct ExclusiveRequest {
    run_dir: PathBuf,
    quarantine_shm: bool,
    quarantine_journal: bool,
    checkpoint_truncate: bool,
    #[cfg(feature = "testkit")]
    after_raw_copies: Option<WindowHook>,
    #[cfg(feature = "testkit")]
    after_exclusive_lock: Option<WindowHook>,
    #[cfg(feature = "testkit")]
    after_evidence_check: Option<WindowHook>,
}

impl ExclusiveRequest {
    /// A request with every action off. `run_dir` is the run directory the engine created under
    /// `<db dir>/.recovery/`.
    #[must_use]
    pub fn new(run_dir: PathBuf) -> Self {
        Self {
            run_dir,
            quarantine_shm: false,
            quarantine_journal: false,
            checkpoint_truncate: false,
            #[cfg(feature = "testkit")]
            after_raw_copies: None,
            #[cfg(feature = "testkit")]
            after_exclusive_lock: None,
            #[cfg(feature = "testkit")]
            after_evidence_check: None,
        }
    }

    /// Rename the `-shm` into `quarantine/` under the held lock (`sidecar_mismatch`).
    #[must_use]
    pub fn quarantine_shm(mut self) -> Self {
        self.quarantine_shm = true;
        self
    }

    /// Let `SQLite` roll a hot `-journal` back in step (c) and report it (`journal_sidecar_present`).
    /// The `raw/` copy is the evidence; the window renames nothing.
    #[must_use]
    pub fn quarantine_journal(mut self) -> Self {
        self.quarantine_journal = true;
        self
    }

    /// Run `PRAGMA wal_checkpoint(TRUNCATE)` and require it to complete (`truncated_wal`).
    #[must_use]
    pub fn checkpoint_truncate(mut self) -> Self {
        self.checkpoint_truncate = true;
        self
    }

    /// Run `hook` with the database path between steps (b) and (c), so a test can change a live
    /// file deterministically (PRD §4 D58 criterion (f6)).
    #[cfg(feature = "testkit")]
    #[must_use]
    pub fn with_after_raw_copies(mut self, hook: Box<dyn Fn(&Path) + Send + Sync>) -> Self {
        self.after_raw_copies = Some(hook);
        self
    }

    /// Run `hook` with the database path once step (c)'s `locking_mode=EXCLUSIVE` holds the
    /// database and before the evidence check, so a test can observe the window's locks before the
    /// check opens the `-wal` and `-shm` (PRD §4 D58 criterion (n), cell fd7, point P1).
    #[cfg(feature = "testkit")]
    #[must_use]
    pub fn with_after_exclusive_lock(mut self, hook: Box<dyn Fn(&Path) + Send + Sync>) -> Self {
        self.after_exclusive_lock = Some(hook);
        self
    }

    /// Run `hook` with the database path once the evidence check has passed, so a test can observe
    /// that the check kept the window's lock (PRD §4 D58 criterion (n), cell fd7, point P2).
    #[cfg(feature = "testkit")]
    #[must_use]
    pub fn with_after_evidence_check(mut self, hook: Box<dyn Fn(&Path) + Send + Sync>) -> Self {
        self.after_evidence_check = Some(hook);
        self
    }
}

impl fmt::Debug for ExclusiveRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("ExclusiveRequest");
        debug
            .field("run_dir", &self.run_dir)
            .field("quarantine_shm", &self.quarantine_shm)
            .field("quarantine_journal", &self.quarantine_journal)
            .field("checkpoint_truncate", &self.checkpoint_truncate);
        #[cfg(feature = "testkit")]
        debug.field("after_raw_copies", &self.after_raw_copies.is_some());
        #[cfg(feature = "testkit")]
        debug.field("after_exclusive_lock", &self.after_exclusive_lock.is_some());
        #[cfg(feature = "testkit")]
        debug.field("after_evidence_check", &self.after_evidence_check.is_some());
        debug.finish()
    }
}

/// Why [`exclusive_window`] refused. Each variant has one stable [`code`](Self::code).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExclusiveRefusal {
    /// Another handle holds `.write.lock` (step (a)).
    WriteLockHeld,
    /// This process has a libsql connection open on the database file (step (a2)).
    ProcessHoldsConnection,
    /// Step (c) failed with `SQLITE_BUSY`, so another connection has the database open.
    DatabaseOpenElsewhere,
    /// A live file differs from what step (b) recorded.
    EvidenceChanged,
    /// The database or a present sidecar is a symlink or not a regular file (step (b), before
    /// any copy).
    NotARegularFile,
    /// Step (c) failed with a result other than `SQLITE_BUSY`.
    UnexpectedOpenResult {
        /// The raw, possibly extended, `SQLite` result code.
        code: i32,
    },
    /// `PRAGMA wal_checkpoint(TRUNCATE)` did not report 0 in its first column.
    CheckpointBusy,
    /// `VACUUM INTO` could not write the snapshot.
    SnapshotFailed,
}

impl ExclusiveRefusal {
    /// The stable snake-case code of the refusal.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::WriteLockHeld => "write_lock_held",
            Self::ProcessHoldsConnection => "process_holds_connection",
            Self::DatabaseOpenElsewhere => "database_open_elsewhere",
            Self::EvidenceChanged => "evidence_changed",
            Self::NotARegularFile => "not_a_regular_file",
            Self::UnexpectedOpenResult { .. } => "unexpected_open_result",
            Self::CheckpointBusy => "checkpoint_busy",
            Self::SnapshotFailed => "snapshot_failed",
        }
    }
}

/// What one [`exclusive_window`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExclusiveOutcome {
    /// The refusal, if any. No step after the refusing one ran.
    pub refused: Option<ExclusiveRefusal>,
    /// Each `<run_dir>/raw/<file>` copy with its byte count, taken while no libsql connection was
    /// open in this process.
    pub raw_copies: Vec<(PathBuf, u64)>,
    /// `<run_dir>/snapshot.db` once step (d) wrote it.
    pub snapshot: Option<PathBuf>,
    /// Each `(from, to)` rename. The window never unlinks.
    pub renamed: Vec<(PathBuf, PathBuf)>,
    /// Whether `PRAGMA wal_checkpoint(TRUNCATE)` ran and completed.
    pub checkpoint_truncated: bool,
    /// Whether a `-journal` step (b) copied is gone after the close: `SQLite` rolled it back and
    /// unlinked it. Set only for `quarantine_journal`.
    pub journal_rolled_back: bool,
}

/// Whether the database at `db_path` is LOST (PRD §4 D58 clause (7)). The file is absent or 0
/// bytes, and `.write.lock` exists beside it.
///
/// It uses `stat` only and never opens either file.
///
/// # Errors
///
/// A `stat` failure other than an absent file.
pub fn lost_database(db_path: &Path) -> io::Result<bool> {
    let absent_or_empty = match fs::metadata(db_path) {
        Ok(meta) => meta.len() == 0,
        Err(error) if error.kind() == ErrorKind::NotFound => true,
        Err(error) => return Err(error),
    };
    if !absent_or_empty {
        return Ok(false);
    }
    write_lock_path(db_path).try_exists()
}

/// The number of open libsql connections in THIS process to the database file at `db_path`.
///
/// Every file-backed connection the backend opens joins a process-wide registry keyed by the
/// file's device and inode (its canonical path on Windows) and leaves it when it closes. An absent
/// file has none.
///
/// # Errors
///
/// A `stat` (Windows `canonicalize`) failure other than an absent file.
pub fn live_connections_to(db_path: &Path) -> io::Result<usize> {
    match FileKey::of(db_path) {
        Ok(key) => Ok(registered_connections(&key)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}

/// Whether another handle holds `.write.lock` right now (PRD §4 D58 clause (2)).
///
/// It opens `.write.lock` without `create`, so an absent file reads as not held and stays absent,
/// because the file is the lost-database marker. It makes one non-blocking `try_lock` and releases
/// the lock at once.
///
/// # Errors
///
/// [`StorageError::RecoveryIo`] when the file exists but cannot be opened, or the lock call fails.
pub fn probe_write_lock(db_path: &Path) -> Result<bool, StorageError> {
    let lock_path = write_lock_path(db_path);
    let file = match OpenOptions::new().read(true).open(&lock_path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(recovery_io(&lock_path, source)),
    };
    let acquired = try_lock(&file, &lock_path)?;
    if acquired {
        let _ = file.unlock();
    }
    Ok(!acquired)
}

/// Prove exclusive access to the database at `db_path` and run `request`'s pre-open repairs (PRD
/// §4 D58 clause (6)). The module docs list the steps.
///
/// # Errors
///
/// - [`StorageError::RecoveryIo`] when the database file is absent at step (a), or a `stat`, copy,
///   comparison or rename fails.
/// - [`StorageError::RecoveryPathOutsideWorkspace`] when the run directory is not a real
///   directory under a real `<db dir>/.recovery/`, or a symlink lies between them.
/// - [`StorageError::Backend`] when libsql fails without a `SQLite` result code.
pub async fn exclusive_window(
    db_path: &Path,
    request: &ExclusiveRequest,
) -> Result<ExclusiveOutcome, StorageError> {
    let mut outcome = ExclusiveOutcome::default();
    let refused = run_window(db_path, request, &mut outcome).await?;
    outcome.refused = refused;
    Ok(outcome)
}

/// The window's steps. Returns the refusal that stopped it, if any. Locals drop in reverse order,
/// so every exit closes the connection before it releases `.write.lock`.
async fn run_window(
    db_path: &Path,
    request: &ExclusiveRequest,
    outcome: &mut ExclusiveOutcome,
) -> Result<Option<ExclusiveRefusal>, StorageError> {
    // (a) Every exclusive action has a database file, so the window never creates the marker for
    // a missing one.
    fs::symlink_metadata(db_path).map_err(|source| recovery_io(db_path, source))?;
    let Some(_lock) = HeldWriteLock::take(&write_lock_path(db_path))? else {
        return Ok(Some(ExclusiveRefusal::WriteLockHeld));
    };

    // (a2) Step (b) closes descriptors on the database file, which must not cancel the POSIX locks
    // of a connection in this process.
    if live_connections_to(db_path).map_err(|source| recovery_io(db_path, source))? > 0 {
        return Ok(Some(ExclusiveRefusal::ProcessHoldsConnection));
    }
    confine_run_dir(db_path, &request.run_dir)?;

    // (b)
    let recorded = match take_raw_copies(db_path, &request.run_dir).await? {
        RawCopies::Taken(recorded) => recorded,
        RawCopies::Refused(refusal) => return Ok(Some(refusal)),
    };
    outcome.raw_copies = recorded
        .iter()
        .map(|file| (file.raw.clone(), file.bytes))
        .collect();
    #[cfg(feature = "testkit")]
    if let Some(hook) = &request.after_raw_copies {
        hook(db_path);
    }

    // (c) Without `SQLITE_OPEN_CREATE` the window never creates a database file.
    let database = match Builder::new_local(db_path)
        .flags(OpenFlags::SQLITE_OPEN_READ_WRITE)
        .build()
        .await
    {
        Ok(database) => database,
        Err(error) => return open_refusal(error).map(Some),
    };
    let conn = match CloseOnceConnection::connect_raw(&database, Some(db_path)).await? {
        Ok(conn) => conn,
        Err(error) => return open_refusal(error).map(Some),
    };
    if let Err(error) = lock_exclusively(&conn).await {
        return open_refusal(error).map(Some);
    }
    #[cfg(feature = "testkit")]
    if let Some(hook) = &request.after_exclusive_lock {
        hook(db_path);
    }
    if check_evidence(db_path, recorded).await? {
        return Ok(Some(ExclusiveRefusal::EvidenceChanged));
    }
    #[cfg(feature = "testkit")]
    if let Some(hook) = &request.after_evidence_check {
        hook(db_path);
    }

    // (d)
    if let Some(refusal) = snapshot_and_checkpoint(&conn, request, outcome).await? {
        return Ok(Some(refusal));
    }
    if request.quarantine_shm {
        quarantine(&Member::Shm.path(db_path), &request.run_dir, outcome)?;
    }

    // (e) The close lets `SQLite` checkpoint and delete the `-wal`, and unlink a hot `-journal`
    // it rolled back in (c). A journal still present is left in place for the final
    // classification to report.
    drop(conn);
    drop(database);
    if request.quarantine_journal {
        let journal = Member::Journal.path(db_path);
        let copied = outcome
            .raw_copies
            .iter()
            .any(|(path, _)| path.file_name() == journal.file_name());
        let present = fs::symlink_metadata(&journal)
            .map(|_| true)
            .or_else(|error| match error.kind() {
                ErrorKind::NotFound => Ok(false),
                _ => Err(recovery_io(&journal, error)),
            })?;
        outcome.journal_rolled_back = copied && !present;
    }
    Ok(None)
}

/// `<db dir>/.write.lock`, derived as [`LibsqlStorage::open_local`](crate::LibsqlStorage) derives
/// it.
fn write_lock_path(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(WRITE_LOCK_FILE_NAME)
}

/// A [`StorageError::RecoveryIo`] for `path`.
fn recovery_io(path: &Path, source: io::Error) -> StorageError {
    StorageError::RecoveryIo {
        path: path.to_path_buf(),
        source,
    }
}

/// One non-blocking exclusive `try_lock`. `Ok(false)` means another handle holds the lock.
fn try_lock(file: &File, lock_path: &Path) -> Result<bool, StorageError> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(source)) => Err(recovery_io(lock_path, source)),
    }
}

/// `.write.lock` held by the window. Dropping it releases the lock.
struct HeldWriteLock(File);

impl HeldWriteLock {
    /// Take `.write.lock` with timeout 0, opening it as the D31 `WriteLock` does. `None` means
    /// another handle holds it.
    fn take(lock_path: &Path) -> Result<Option<Self>, StorageError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .map_err(|source| recovery_io(lock_path, source))?;
        if try_lock(&file, lock_path)? {
            Ok(Some(Self(file)))
        } else {
            Ok(None)
        }
    }
}

impl Drop for HeldWriteLock {
    fn drop(&mut self) {
        // Closing the descriptor releases the lock too.
        let _ = self.0.unlock();
    }
}

/// Require `run_dir` to be a real directory per `lstat` under `<db dir>/.recovery/`, confined by
/// identity ([`confine`]).
fn confine_run_dir(db_path: &Path, run_dir: &Path) -> Result<(), StorageError> {
    confine(db_path, run_dir)?;
    if real_dir(run_dir)?.is_none() {
        return Err(StorageError::RecoveryPathOutsideWorkspace {
            path: run_dir.to_path_buf(),
        });
    }
    Ok(())
}

/// The database file and its three sidecars, in the order the window copies and moves them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Member {
    Database,
    Wal,
    Shm,
    Journal,
}

impl Member {
    const ALL: [Self; 4] = [Self::Database, Self::Wal, Self::Shm, Self::Journal];

    /// The member's path beside `db_path`.
    ///
    /// The suffix literals are `SQLite`'s fixed sidecar names. Above L2 their owner is
    /// `unblock_sync::SIDECAR_SUFFIXES` (PRD §4 D5 amendment, `ub-c15`); this crate (L2) cannot
    /// depend on `unblock-sync` (L3, NFR-15), so it keeps its own private copy here.
    fn path(self, db_path: &Path) -> PathBuf {
        let suffix = match self {
            Self::Database => "",
            Self::Wal => "-wal",
            Self::Shm => "-shm",
            Self::Journal => "-journal",
        };
        let mut path = db_path.as_os_str().to_owned();
        path.push(suffix);
        PathBuf::from(path)
    }
}

/// The metadata step (b) records for a live file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    len: u64,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    /// Seconds and nanoseconds.
    #[cfg(unix)]
    mtime: (i64, i64),
    /// Seconds and nanoseconds.
    #[cfg(unix)]
    ctime: (i64, i64),
    /// 100 ns `FILETIME` ticks.
    #[cfg(windows)]
    last_write_time: u64,
    /// 100 ns `FILETIME` ticks.
    #[cfg(windows)]
    creation_time: u64,
}

impl Stamp {
    fn of(meta: &Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            Self {
                len: meta.len(),
                dev: meta.dev(),
                ino: meta.ino(),
                mtime: (meta.mtime(), meta.mtime_nsec()),
                ctime: (meta.ctime(), meta.ctime_nsec()),
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt as _;
            Self {
                len: meta.len(),
                last_write_time: meta.last_write_time(),
                creation_time: meta.creation_time(),
            }
        }
    }

    /// Whether both stamps name the same file, whatever its content.
    fn same_file(&self, other: &Self) -> bool {
        #[cfg(unix)]
        {
            self.dev == other.dev && self.ino == other.ino
        }
        #[cfg(windows)]
        {
            self.creation_time == other.creation_time
        }
    }
}

/// One family file that step (b) found and copied.
struct Recorded {
    member: Member,
    live: PathBuf,
    raw: PathBuf,
    bytes: u64,
    stamp: Stamp,
}

/// Run `work` on tokio's blocking pool. A panicked or cancelled task is a
/// [`StorageError::RecoveryIo`] on `path`.
async fn blocking<T, F>(path: &Path, work: F) -> Result<T, StorageError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, StorageError> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|join| recovery_io(path, io::Error::other(join.to_string())))?
}

/// What step (b) produced.
enum RawCopies {
    /// Every present family file, copied.
    Taken(Vec<Recorded>),
    /// A family file is not a regular file, or vanished between its `stat` and its copy.
    Refused(ExclusiveRefusal),
}

/// Step (b).
async fn take_raw_copies(db_path: &Path, run_dir: &Path) -> Result<RawCopies, StorageError> {
    let raw_dir = run_dir.join(RAW_DIR_NAME);
    let db = db_path.to_path_buf();
    blocking(run_dir, move || copy_family(&db, &raw_dir)).await
}

/// Copy every present family file into `raw_dir`, once each is known to be a regular file. Each
/// `stat` comes before its copy, so a change during the copy shows up at the evidence check.
fn copy_family(db_path: &Path, raw_dir: &Path) -> Result<RawCopies, StorageError> {
    if !family_is_regular(db_path)? {
        return Ok(RawCopies::Refused(ExclusiveRefusal::NotARegularFile));
    }
    create_private_dir(raw_dir).map_err(|source| recovery_io(raw_dir, source))?;
    let mut recorded = Vec::new();
    for member in Member::ALL {
        let live = member.path(db_path);
        let meta = match fs::symlink_metadata(&live) {
            Ok(meta) if meta.file_type().is_file() => meta,
            Ok(_) => return Ok(RawCopies::Refused(ExclusiveRefusal::NotARegularFile)),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                if member == Member::Database {
                    return Ok(RawCopies::Refused(ExclusiveRefusal::EvidenceChanged));
                }
                continue;
            }
            Err(source) => return Err(recovery_io(&live, source)),
        };
        let raw = raw_dir.join(file_name(&live)?);
        let bytes = match fs::copy(&live, &raw) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(RawCopies::Refused(ExclusiveRefusal::EvidenceChanged));
            }
            Err(source) => return Err(recovery_io(&live, source)),
        };
        restrict_file(&raw).map_err(|source| recovery_io(&raw, source))?;
        recorded.push(Recorded {
            member,
            live,
            raw,
            bytes,
            stamp: Stamp::of(&meta),
        });
    }
    Ok(RawCopies::Taken(recorded))
}

/// Whether the database and every present sidecar is a regular file per `lstat`. A symlink,
/// directory, FIFO or device is not.
fn family_is_regular(db_path: &Path) -> Result<bool, StorageError> {
    for member in Member::ALL {
        let live = member.path(db_path);
        match fs::symlink_metadata(&live) {
            Ok(meta) if meta.file_type().is_file() => {}
            Ok(_) => return Ok(false),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(source) => return Err(recovery_io(&live, source)),
        }
    }
    Ok(true)
}

/// The final component of `path`.
fn file_name(path: &Path) -> Result<&std::ffi::OsStr, StorageError> {
    path.file_name()
        .ok_or_else(|| recovery_io(path, io::Error::from(ErrorKind::InvalidInput)))
}

/// Step (c)'s statements. The close guard is prepared and never stepped, so
/// `PRAGMA locking_mode=EXCLUSIVE` is the first statement that runs.
async fn lock_exclusively(conn: &Connection) -> Result<(), libsql::Error> {
    conn.busy_timeout(Duration::ZERO)?;
    let mut rows = conn.query("PRAGMA locking_mode=EXCLUSIVE", ()).await?;
    while rows.next().await?.is_some() {}
    drop(rows);
    conn.execute("BEGIN EXCLUSIVE", ()).await?;
    conn.execute("COMMIT", ()).await?;
    Ok(())
}

/// The refusal for a failed step (c), read on the raw `SQLite` result code before
/// [`map_libsql_err`] absorbs it. `SQLITE_BUSY` means another connection holds the database.
///
/// # Errors
///
/// [`StorageError::Backend`] when libsql reports no result code.
fn open_refusal(error: libsql::Error) -> Result<ExclusiveRefusal, StorageError> {
    let Some(code) = raw_code(&error) else {
        return Err(map_libsql_err(error));
    };
    if code & 0xff == SQLITE_BUSY {
        Ok(ExclusiveRefusal::DatabaseOpenElsewhere)
    } else {
        Ok(ExclusiveRefusal::UnexpectedOpenResult { code })
    }
}

/// The raw `SQLite` result code of a libsql failure. libsql reports a failed `sqlite3_open_v2`
/// only in the text of `ConnectionFailed`, which ends with the code.
fn raw_code(error: &libsql::Error) -> Option<i32> {
    match error {
        libsql::Error::SqliteFailure(code, _) => Some(*code),
        libsql::Error::ConnectionFailed(message) => {
            message.rsplit(": ").next()?.trim().parse().ok()
        }
        _ => None,
    }
}

/// The evidence check of step (c), on a blocking thread. `Ok(true)` means another process changed
/// a file between steps (a) and (c).
async fn check_evidence(db_path: &Path, recorded: Vec<Recorded>) -> Result<bool, StorageError> {
    let db = db_path.to_path_buf();
    blocking(db_path, move || evidence_changed(&db, &recorded)).await
}

/// Whether any live family file differs from what step (b) recorded.
///
/// - Every recorded file must still exist, and every present one must be a regular file per
///   `lstat`.
/// - The database file must keep its device, inode, size and nanosecond mtime and ctime.
/// - The `-wal` and `-shm` must match their raw copies byte for byte. `SQLite` holds no POSIX lock
///   on either, and the exclusive connection maps no `-shm`, so reading them opens no descriptor on
///   the database file.
/// - A `-wal` that step (b) did not see is expected when it is empty, because step (c)'s own open
///   creates it.
/// - A `-journal` that step (b) saw lets `SQLite`'s hot-journal rollback rewrite the database file
///   and the journal, so only the database file's identity is checked then.
///
/// Any other file that appears counts as changed. On a filesystem with coarse timestamps (HFS+,
/// FAT, some NFS mounts) a same-size change to the database file within one tick goes undetected.
fn evidence_changed(db_path: &Path, recorded: &[Recorded]) -> Result<bool, StorageError> {
    let journal_seen = recorded.iter().any(|file| file.member == Member::Journal);
    for member in Member::ALL {
        let live = member.path(db_path);
        let now = match fs::symlink_metadata(&live) {
            Ok(meta) if meta.file_type().is_file() => Some(meta),
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(source) => return Err(recovery_io(&live, source)),
        };
        let before = recorded.iter().find(|file| file.member == member);
        let unchanged = match (member, before, now) {
            (_, None, None) | (Member::Journal, Some(_), _) => true,
            (Member::Database, Some(before), Some(now)) => {
                database_unchanged(before, &Stamp::of(&now), journal_seen)
            }
            (Member::Wal | Member::Shm, Some(before), Some(_)) => {
                sidecar_unchanged(&before.live, &before.raw)?
            }
            (Member::Wal, None, Some(now)) => now.len() == 0,
            (_, Some(_), None) | (_, None, Some(_)) => false,
        };
        if !unchanged {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether the database file still matches step (b).
fn database_unchanged(before: &Recorded, now: &Stamp, journal_seen: bool) -> bool {
    if journal_seen {
        return before.stamp.same_file(now);
    }
    before.stamp == *now && database_bytes_unchanged(before)
}

/// Windows re-reads the database file against its raw copy. Its byte-range locks belong to their
/// handle, so closing this handle releases nothing, and the POSIX hazard of step (a2) does not
/// exist there. Any read error counts as changed.
#[cfg(windows)]
fn database_bytes_unchanged(before: &Recorded) -> bool {
    matches!(
        same_bytes(&before.live, &before.raw, Some(LOCK_BYTE_RANGE)),
        Ok(true)
    )
}

/// Unix never opens the database file after step (b), because closing that descriptor would cancel
/// the window's own POSIX locks.
#[cfg(not(windows))]
fn database_bytes_unchanged(_before: &Recorded) -> bool {
    true
}

/// Whether a `-wal` or `-shm` still holds the bytes of its raw copy. A file that vanished or
/// shrank during the read counts as changed.
fn sidecar_unchanged(live: &Path, raw: &Path) -> Result<bool, StorageError> {
    match same_bytes(live, raw, None) {
        Ok(same) => Ok(same),
        Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::UnexpectedEof) => {
            Ok(false)
        }
        Err(source) => Err(recovery_io(live, source)),
    }
}

/// Whether `live` holds exactly the bytes of `copy`, with the `skip` range `(start, end)` left
/// out of both.
fn same_bytes(live: &Path, copy: &Path, skip: Option<(u64, u64)>) -> io::Result<bool> {
    let mut live = File::open(live)?;
    let mut copy = File::open(copy)?;
    let len = live.metadata()?.len();
    if len != copy.metadata()?.len() {
        return Ok(false);
    }
    let mut live_buf = vec![0_u8; CHUNK_LEN];
    let mut copy_buf = vec![0_u8; CHUNK_LEN];
    let mut offset = 0_u64;
    while offset < len {
        if let Some((start, end)) = skip
            && (start..end).contains(&offset)
        {
            offset = end.min(len);
            live.seek(SeekFrom::Start(offset))?;
            copy.seek(SeekFrom::Start(offset))?;
            continue;
        }
        let stop = match skip {
            Some((start, _)) if start > offset => start.min(len),
            _ => len,
        };
        let n = usize::try_from(stop - offset).map_or(CHUNK_LEN, |left| left.min(CHUNK_LEN));
        live.read_exact(&mut live_buf[..n])?;
        copy.read_exact(&mut copy_buf[..n])?;
        if live_buf[..n] != copy_buf[..n] {
            return Ok(false);
        }
        offset += u64::try_from(n).map_err(io::Error::other)?;
    }
    Ok(true)
}

/// Step (d). Returns the refusal that stops the window, if any.
async fn snapshot_and_checkpoint(
    conn: &Connection,
    request: &ExclusiveRequest,
    outcome: &mut ExclusiveOutcome,
) -> Result<Option<ExclusiveRefusal>, StorageError> {
    let snapshot = request.run_dir.join(SNAPSHOT_FILE_NAME);
    let target = snapshot.to_str().ok_or_else(|| {
        recovery_io(
            &snapshot,
            io::Error::new(ErrorKind::InvalidInput, "the snapshot path is not UTF-8"),
        )
    })?;
    if conn.execute("VACUUM INTO ?1", [target]).await.is_err() {
        return Ok(Some(ExclusiveRefusal::SnapshotFailed));
    }
    restrict_file(&snapshot).map_err(|source| recovery_io(&snapshot, source))?;
    outcome.snapshot = Some(snapshot);
    if request.checkpoint_truncate {
        if !checkpoint_completed(conn).await {
            return Ok(Some(ExclusiveRefusal::CheckpointBusy));
        }
        outcome.checkpoint_truncated = true;
    }
    Ok(None)
}

/// Whether `PRAGMA wal_checkpoint(TRUNCATE)` completed. Its first column is 0 then, and any other
/// result counts as busy.
async fn checkpoint_completed(conn: &Connection) -> bool {
    let Ok(mut rows) = conn.query("PRAGMA wal_checkpoint(TRUNCATE)", ()).await else {
        return false;
    };
    let Ok(Some(row)) = rows.next().await else {
        return false;
    };
    matches!(row.get::<i64>(0), Ok(0))
}

/// Rename `live` into `<run_dir>/quarantine/` when it exists. An existing target refuses, so
/// nothing is ever overwritten.
fn quarantine(
    live: &Path,
    run_dir: &Path,
    outcome: &mut ExclusiveOutcome,
) -> Result<(), StorageError> {
    match fs::symlink_metadata(live) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(recovery_io(live, source)),
    }
    let dir = run_dir.join(QUARANTINE_DIR_NAME);
    match create_private_dir(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(source) => return Err(recovery_io(&dir, source)),
    }
    let target = dir.join(file_name(live)?);
    if target
        .try_exists()
        .map_err(|source| recovery_io(&target, source))?
    {
        return Err(recovery_io(
            &target,
            io::Error::from(ErrorKind::AlreadyExists),
        ));
    }
    fs::rename(live, &target).map_err(|source| recovery_io(live, source))?;
    restrict_file(&target).map_err(|source| recovery_io(&target, source))?;
    outcome.renamed.push((live.to_path_buf(), target));
    Ok(())
}
