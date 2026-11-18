//! PRD §4 D58 criterion (f), MEASURED, plus the storage halves of criteria (g), (h) and (n) and the
//! three other local-file primitives (`lost_database`, `live_connections_to`, `probe_write_lock`).
//!
//! The `f*` cells measure the exclusivity proof against a REAL second process, the
//! `exclusive_window_holder` helper `[[bin]]`. It holds a libsql reader that has served one read,
//! the `.write.lock`, or a killed write transaction's hot `-journal`, and it runs the (f5) control.
//! The cells run on Linux in `storage-testkit` and on macOS in `exclusivity-macos`, and on Linux
//! again in `test (workspace)`, where `unblock-fuzz` turns the storage `testkit` on. Windows takes
//! another VFS locking path and stays unmeasured, so the file is Unix-only. The `f6` cells need the
//! testkit-only `with_after_raw_copies` hook.
//!
//! The `fd*` cells are criterion (n)'s descriptor-close lock-loss cells (`ub-2vp`). The holder runs
//! the v1.0.x header probe, copied verbatim, beside its live libsql connections, and
//! `common::posix_locks` reads who holds `SQLite`'s SHARED byte range from this process (`F_GETLK`,
//! plus `/proc/locks` on Linux). A fresh observer process trying `locking_mode=EXCLUSIVE` is the
//! behavioural witness. `fd7` observes the real window through the testkit-only
//! `with_after_exclusive_lock` and `with_after_evidence_check` hooks. Each observation prints one
//! `UB2VP-MEASURE` evidence line.
#![cfg(all(unix, feature = "testkit"))]

mod common;

use std::fs;
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};

use common::posix_locks::{self, LockKind, Observation};

use tempfile::TempDir;
use unblock_model::ListFilters;
use unblock_storage::{
    DEFAULT_WRITE_LOCK_TIMEOUT_MS, ExclusiveOutcome, ExclusiveRefusal, ExclusiveRequest,
    LibsqlStorage, RepairRequest, Storage, StorageError, exclusive_window, live_connections_to,
    lost_database, probe_write_lock,
};

const HOLDER: &str = env!("CARGO_BIN_EXE_exclusive_window_holder");

/// The first 8 bytes of every valid rollback-journal header (`SQLite` `aJournalMagic`).
const JOURNAL_MAGIC: [u8; 8] = [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7];

/// A workspace directory with `unblock.db` and one run directory under `.recovery/`.
struct Workspace {
    dir: TempDir,
    db: PathBuf,
    run_dir: PathBuf,
}

impl Workspace {
    /// No database file yet.
    fn empty() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("unblock.db");
        let run_dir = dir
            .path()
            .join(".recovery")
            .join("20261113T000000Z-0badc0de");
        fs::create_dir_all(&run_dir).expect("create the run directory");
        Self { dir, db, run_dir }
    }

    /// A migrated database whose store has closed, so only `unblock.db` and `.write.lock` remain.
    async fn migrated() -> Self {
        let workspace = Self::empty();
        let store = LibsqlStorage::open_local(&workspace.db, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
            .await
            .expect("open_local");
        store.migrate().await.expect("migrate");
        drop(store);
        assert!(workspace.marker().exists(), "migrate writes the marker");
        assert!(
            !workspace.sidecar("-wal").exists() && !workspace.sidecar("-shm").exists(),
            "the last close deletes the -wal and the -shm"
        );
        workspace
    }

    fn sidecar(&self, suffix: &str) -> PathBuf {
        self.dir.path().join(format!("unblock.db{suffix}"))
    }

    fn marker(&self) -> PathBuf {
        self.dir.path().join(".write.lock")
    }

    fn raw(&self, name: &str) -> PathBuf {
        self.run_dir.join("raw").join(name)
    }

    fn quarantined(&self, name: &str) -> PathBuf {
        self.run_dir.join("quarantine").join(name)
    }

    fn request(&self) -> ExclusiveRequest {
        ExclusiveRequest::new(self.run_dir.clone())
    }
}

/// A running `exclusive_window_holder`. It owns the child's stdin and stdout for its whole life,
/// so a reply written after `READY` never meets a closed pipe. Dropping it kills the process and
/// reaps it.
struct Holder {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    mode: String,
}

impl Holder {
    /// Spawn `mode` on `db` and wait for its `READY` line.
    fn spawn(mode: &str, db: &Path) -> Self {
        Self::spawn_with(mode, db, &[])
    }

    /// Spawn `mode` on `db` with `extra` arguments and wait for its `READY` line. The `Holder`
    /// exists before `READY` is asserted, so a mismatch still kills and reaps the child.
    fn spawn_with(mode: &str, db: &Path, extra: &[&Path]) -> Self {
        let mut child = Command::new(HOLDER)
            .arg(mode)
            .arg(db)
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn exclusive_window_holder");
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().expect("holder stdout"));
        let mut holder = Self {
            child,
            stdin,
            stdout,
            mode: mode.to_owned(),
        };
        let line = holder.read_line();
        assert_eq!(line, "READY", "the {mode} holder never got ready");
        holder
    }

    /// The holder's pid.
    fn pid(&self) -> i32 {
        i32::try_from(self.child.id()).expect("a pid fits i32")
    }

    /// One line of the holder's stdout, without its line ending. EOF panics with the mode name.
    fn read_line(&mut self) -> String {
        let mut line = String::new();
        let read = self
            .stdout
            .read_line(&mut line)
            .expect("read the holder's stdout");
        assert!(read > 0, "the {} holder closed its stdout", self.mode);
        line.trim_end().to_owned()
    }

    /// Write `line` to the holder, then read exactly one reply line.
    fn send(&mut self, line: &str) -> String {
        let stdin = self.stdin.as_mut().expect("the holder's stdin is open");
        writeln!(stdin, "{line}").expect("write to the holder");
        stdin.flush().expect("flush the holder's stdin");
        self.read_line()
    }

    /// Close the holder's stdin and wait for it. The holder drops its store gracefully.
    fn quit(mut self) -> ExitStatus {
        drop(self.stdin.take());
        self.child.wait().expect("reap the holder")
    }

    /// SIGKILL the holder: no destructor runs, so an open transaction is abandoned mid-flight.
    fn kill(mut self) {
        self.child.kill().expect("SIGKILL the holder");
        let status = self.child.wait().expect("reap the holder");
        assert!(!status.success(), "a SIGKILLed process never exits 0");
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The names in `dir`, sorted. An absent directory has none.
fn names(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// A recognisable byte pattern of `len` bytes.
fn pattern(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| u8::try_from(i % 251).expect("below 251"))
        .collect()
}

// ---- (f) exclusivity, measured ----

/// (f1) A second process holding an open connection that has served one read makes step (c) fail
/// with `SQLITE_BUSY`. The window refuses `database_open_elsewhere` and renames nothing, although
/// the request asked for the reader's live `-shm`.
#[tokio::test]
async fn f1_second_process_reader_refuses_database_open_elsewhere() {
    let workspace = Workspace::migrated().await;
    let reader = Holder::spawn("reader", &workspace.db);
    assert!(
        workspace.sidecar("-shm").exists(),
        "the reader's WAL open maps a -shm"
    );

    let outcome = exclusive_window(
        &workspace.db,
        &workspace.request().quarantine_shm().checkpoint_truncate(),
    )
    .await
    .expect("window");

    assert_eq!(
        outcome.refused,
        Some(ExclusiveRefusal::DatabaseOpenElsewhere)
    );
    assert_eq!(
        outcome.refused.map(|refusal| refusal.code()),
        Some("database_open_elsewhere")
    );
    assert!(outcome.renamed.is_empty(), "renamed {:?}", outcome.renamed);
    assert_eq!(outcome.snapshot, None);
    assert!(!outcome.checkpoint_truncated);
    assert!(!workspace.run_dir.join("quarantine").exists());
    assert!(
        workspace.sidecar("-shm").exists(),
        "the live -shm stays put"
    );
    assert!(
        workspace.sidecar("-wal").exists(),
        "the live -wal stays put"
    );
    assert_eq!(live_connections_to(&workspace.db).expect("live"), 0);
    drop(reader);
}

/// (f2) A second process holding `.write.lock` makes the window refuse `write_lock_held` before it
/// reads any file, so no `raw/` directory exists.
#[tokio::test]
async fn f2_second_process_write_lock_refuses_write_lock_held_before_any_read() {
    let workspace = Workspace::migrated().await;
    let holder = Holder::spawn("write-lock", &workspace.db);

    let outcome = exclusive_window(&workspace.db, &workspace.request().checkpoint_truncate())
        .await
        .expect("window");

    assert_eq!(outcome.refused, Some(ExclusiveRefusal::WriteLockHeld));
    assert!(outcome.raw_copies.is_empty());
    assert_eq!(outcome.snapshot, None);
    assert!(
        names(&workspace.run_dir).is_empty(),
        "nothing in the run directory"
    );
    drop(holder);
}

/// (f3) With no other process the window succeeds. The raw copy holds the database bytes, the
/// snapshot passes `integrity_check`, the `-wal` the window's own open created is gone, and the
/// lock and the registry are free again.
#[tokio::test]
async fn f3_no_other_process_succeeds() {
    let workspace = Workspace::migrated().await;
    let db_bytes = fs::read(&workspace.db).expect("read the database");

    let outcome = exclusive_window(&workspace.db, &workspace.request().checkpoint_truncate())
        .await
        .expect("window");

    assert_eq!(outcome.refused, None);
    let raw_len = u64::try_from(db_bytes.len()).expect("length");
    assert_eq!(
        outcome.raw_copies,
        vec![(workspace.raw("unblock.db"), raw_len)]
    );
    assert_eq!(
        fs::read(workspace.raw("unblock.db")).expect("raw"),
        db_bytes
    );
    assert!(outcome.checkpoint_truncated);
    assert!(outcome.renamed.is_empty());
    let snapshot = outcome.snapshot.expect("snapshot");
    assert_eq!(snapshot, workspace.run_dir.join("snapshot.db"));
    let snapshot_store = LibsqlStorage::open_local(&snapshot, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open the snapshot");
    assert!(
        snapshot_store
            .integrity_check()
            .await
            .expect("integrity_check")
            .is_empty()
    );
    drop(snapshot_store);

    assert!(workspace.db.exists());
    assert!(
        !workspace.sidecar("-wal").exists(),
        "SQLite deleted the -wal"
    );
    assert_eq!(live_connections_to(&workspace.db).expect("live"), 0);
    assert!(
        !probe_write_lock(&workspace.db).expect("probe"),
        "lock released"
    );
}

/// (f4, n) A store open on THE SAME database in this process refuses `process_holds_connection`
/// before any file is read, so no `raw/` directory exists.
#[tokio::test]
async fn f4_same_process_store_refuses_process_holds_connection_and_creates_no_raw() {
    let workspace = Workspace::migrated().await;
    let store = LibsqlStorage::open_local(&workspace.db, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open_local");
    assert_eq!(live_connections_to(&workspace.db).expect("live"), 2);

    let outcome = exclusive_window(&workspace.db, &workspace.request().checkpoint_truncate())
        .await
        .expect("window");

    assert_eq!(
        outcome.refused,
        Some(ExclusiveRefusal::ProcessHoldsConnection)
    );
    assert!(outcome.raw_copies.is_empty());
    assert!(!workspace.run_dir.join("raw").exists());
    assert!(
        !probe_write_lock(&workspace.db).expect("probe"),
        "lock released"
    );
    drop(store);
}

/// (f4, n) A store open on ANOTHER database does not block the window.
#[tokio::test]
async fn f4_store_on_another_database_does_not_block() {
    let workspace = Workspace::migrated().await;
    let other = Workspace::migrated().await;
    let store = LibsqlStorage::open_local(&other.db, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open_local");
    assert_eq!(live_connections_to(&other.db).expect("live"), 2);
    assert_eq!(live_connections_to(&workspace.db).expect("live"), 0);

    let outcome = exclusive_window(&workspace.db, &workspace.request().checkpoint_truncate())
        .await
        .expect("window");

    assert_eq!(outcome.refused, None);
    assert!(outcome.snapshot.is_some());
    drop(store);
}

/// (f5) The control. `BEGIN EXCLUSIVE` WITHOUT `locking_mode=EXCLUSIVE` commits beside the same
/// WAL reader that the window refuses, because "EXCLUSIVE and IMMEDIATE are the same in WAL mode".
#[tokio::test]
async fn f5_control_begin_exclusive_without_locking_mode_misses_a_wal_reader() {
    let workspace = Workspace::migrated().await;
    let reader = Holder::spawn("reader", &workspace.db);

    let control = Command::new(HOLDER)
        .arg("begin-exclusive")
        .arg(&workspace.db)
        .output()
        .expect("run the control");
    assert!(control.status.success(), "the control process exits 0");
    assert_eq!(
        String::from_utf8_lossy(&control.stdout).trim(),
        "COMMITTED",
        "BEGIN EXCLUSIVE alone does not see the WAL reader"
    );

    let outcome = exclusive_window(&workspace.db, &workspace.request())
        .await
        .expect("window");
    assert_eq!(
        outcome.refused,
        Some(ExclusiveRefusal::DatabaseOpenElsewhere),
        "locking_mode=EXCLUSIVE does see it"
    );
    drop(reader);
}

/// (f6) A database file rewritten between steps (b) and (c) refuses `evidence_changed`.
#[tokio::test]
async fn f6_evidence_changed_when_the_database_is_rewritten() {
    let workspace = Workspace::migrated().await;
    let request = workspace
        .request()
        .checkpoint_truncate()
        .with_after_raw_copies(Box::new(|db: &Path| {
            let mut bytes = fs::read(db).expect("read the database");
            bytes.extend([0_u8; 4096]);
            fs::write(db, bytes).expect("rewrite the database in place");
        }));

    let outcome = exclusive_window(&workspace.db, &request)
        .await
        .expect("window");

    assert_eq!(outcome.refused, Some(ExclusiveRefusal::EvidenceChanged));
    assert_eq!(outcome.raw_copies.len(), 1, "the raw copy was taken");
    assert_eq!(outcome.snapshot, None);
    assert!(!outcome.checkpoint_truncated);
    assert!(outcome.renamed.is_empty());
}

/// (f6) A `-wal` rewritten between steps (b) and (c) refuses `evidence_changed`.
#[tokio::test]
async fn f6_evidence_changed_when_the_wal_is_rewritten() {
    let workspace = Workspace::migrated().await;
    let wal = workspace.sidecar("-wal");
    fs::write(&wal, [1_u8, 2, 3, 4, 5]).expect("write a truncated -wal");
    let hook_wal = wal.clone();
    let request = workspace
        .request()
        .checkpoint_truncate()
        .with_after_raw_copies(Box::new(move |_db: &Path| {
            fs::write(&hook_wal, [5_u8, 4, 3, 2, 1]).expect("rewrite the -wal");
        }));

    let outcome = exclusive_window(&workspace.db, &request)
        .await
        .expect("window");

    assert_eq!(outcome.refused, Some(ExclusiveRefusal::EvidenceChanged));
    assert_eq!(
        fs::read(workspace.raw("unblock.db-wal")).expect("raw -wal"),
        [1_u8, 2, 3, 4, 5]
    );
    assert!(!outcome.checkpoint_truncated);
    assert!(outcome.renamed.is_empty());
}

/// (f6) A recorded file deleted between steps (b) and (c) refuses `evidence_changed`.
#[tokio::test]
async fn f6_evidence_changed_when_a_recorded_file_is_deleted() {
    let workspace = Workspace::migrated().await;
    let shm = workspace.sidecar("-shm");
    fs::write(&shm, pattern(32 * 1024)).expect("write a lone -shm");
    let hook_shm = shm.clone();
    let request = workspace
        .request()
        .quarantine_shm()
        .with_after_raw_copies(Box::new(move |_db: &Path| {
            fs::remove_file(&hook_shm).expect("delete the -shm");
        }));

    let outcome = exclusive_window(&workspace.db, &request)
        .await
        .expect("window");

    assert_eq!(outcome.refused, Some(ExclusiveRefusal::EvidenceChanged));
    assert!(workspace.raw("unblock.db-shm").exists());
    assert!(outcome.renamed.is_empty());
}

/// (f6) A `-wal` that step (b) did not see and step (c)'s own open created is expected.
#[tokio::test]
async fn f6_wal_created_by_the_window_open_is_expected() {
    let workspace = Workspace::migrated().await;
    let wal = workspace.sidecar("-wal");
    let request = workspace
        .request()
        .with_after_raw_copies(Box::new(move |_db: &Path| {
            assert!(!wal.exists(), "no -wal between steps (b) and (c)");
        }));

    let outcome = exclusive_window(&workspace.db, &request)
        .await
        .expect("window");

    assert_eq!(outcome.refused, None);
    assert_eq!(names(&workspace.run_dir.join("raw")), ["unblock.db"]);
    assert!(outcome.snapshot.is_some());
}

// ---- (g) pre-open repairs, storage half ----

/// (g) A lone `-shm` is copied into `raw/`, then renamed into `quarantine/` with its bytes intact.
#[tokio::test]
async fn g_lone_shm_is_renamed_into_quarantine() {
    let workspace = Workspace::migrated().await;
    let shm = workspace.sidecar("-shm");
    let shm_bytes = pattern(32 * 1024);
    fs::write(&shm, &shm_bytes).expect("write a lone -shm");

    let outcome = exclusive_window(&workspace.db, &workspace.request().quarantine_shm())
        .await
        .expect("window");

    assert_eq!(outcome.refused, None);
    let quarantined = workspace.quarantined("unblock.db-shm");
    assert_eq!(outcome.renamed, vec![(shm.clone(), quarantined.clone())]);
    assert_eq!(fs::read(&quarantined).expect("quarantined -shm"), shm_bytes);
    assert_eq!(
        fs::read(workspace.raw("unblock.db-shm")).expect("raw -shm"),
        shm_bytes
    );
    assert!(!shm.exists());
    assert!(!workspace.sidecar("-wal").exists());
    assert!(outcome.snapshot.is_some());
}

/// (g) A 5-byte `-wal` is byte-preserved under `raw/`, the TRUNCATE checkpoint completes, and
/// `SQLite` removes the `-wal` at the window's close. The window never renames it.
#[tokio::test]
async fn g_five_byte_wal_is_preserved_in_raw_and_absent_after() {
    let workspace = Workspace::migrated().await;
    let wal = workspace.sidecar("-wal");
    fs::write(&wal, [1_u8, 2, 3, 4, 5]).expect("write a truncated -wal");

    let outcome = exclusive_window(&workspace.db, &workspace.request().checkpoint_truncate())
        .await
        .expect("window");

    assert_eq!(outcome.refused, None);
    assert!(outcome.checkpoint_truncated);
    assert_eq!(
        fs::read(workspace.raw("unblock.db-wal")).expect("raw -wal"),
        [1_u8, 2, 3, 4, 5]
    );
    assert!(outcome.renamed.is_empty());
    assert!(!wal.exists(), "SQLite deleted the -wal at the close");
    assert!(workspace.db.exists());
}

/// A migrated database left with a hot `-journal` by a killed writer, then one window with
/// `quarantine_journal`. Returns the workspace, the outcome and the pre-rollback journal bytes.
async fn hot_journal_window() -> (Workspace, ExclusiveOutcome, Vec<u8>) {
    let workspace = Workspace::migrated().await;
    Holder::spawn("hot-journal", &workspace.db).kill();
    let journal_bytes =
        fs::read(workspace.sidecar("-journal")).expect("the killed writer left a -journal");
    assert_eq!(
        journal_bytes.get(..8),
        Some(JOURNAL_MAGIC.as_slice()),
        "the journal header was synced, so the journal is hot"
    );
    let outcome = exclusive_window(&workspace.db, &workspace.request().quarantine_journal())
        .await
        .expect("window");
    (workspace, outcome, journal_bytes)
}

/// (g) A hot `-journal` left by a killed writer is rolled back by `SQLite` in step (c), and its
/// pre-rollback bytes are under `raw/`.
#[tokio::test]
async fn g_hot_journal_is_rolled_back_with_its_pre_rollback_bytes_in_raw() {
    let (workspace, outcome, journal_bytes) = hot_journal_window().await;

    assert_eq!(outcome.refused, None);
    assert_eq!(
        fs::read(workspace.raw("unblock.db-journal")).expect("raw -journal"),
        journal_bytes
    );
    let store = LibsqlStorage::open_local(&workspace.db, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("reopen");
    assert!(store.integrity_check().await.expect("integrity").is_empty());
    let filters = ListFilters {
        include_closed: true,
        include_deferred: true,
        ..ListFilters::default()
    };
    assert_eq!(
        store.list_issues(&filters).await.expect("list").len(),
        0,
        "the rollback discarded every uncommitted row"
    );
}

/// (g) `SQLite`'s rollback of a hot `-journal` IS the `quarantine_journal` repair (PRD §4 D58
/// amendment, measured 2026-11-13): the pre-rollback bytes are the `raw/` evidence, the outcome
/// reports the rollback, the window renames nothing, and the `-journal` is absent after the close,
/// like the `-wal`. Removing the `raw/` copy or the report turns this red.
#[tokio::test]
async fn g_hot_journal_rollback_is_the_repair_with_raw_evidence_and_no_journal_after_close() {
    let (workspace, outcome, journal_bytes) = hot_journal_window().await;

    assert_eq!(outcome.refused, None);
    let raw = workspace.raw("unblock.db-journal");
    assert!(
        outcome.raw_copies.iter().any(|(path, bytes)| {
            *path == raw && *bytes == u64::try_from(journal_bytes.len()).expect("len")
        }),
        "the outcome lists the raw -journal copy with its byte count: {:?}",
        outcome.raw_copies
    );
    assert_eq!(
        fs::read(&raw).expect("the raw -journal copy is the evidence"),
        journal_bytes,
        "the raw copy holds the PRE-rollback bytes"
    );
    assert!(
        outcome.journal_rolled_back,
        "the outcome reports the rollback"
    );
    assert!(outcome.renamed.is_empty(), "nothing is renamed");
    assert!(
        !workspace.sidecar("-journal").exists(),
        "SQLite unlinked the rolled-back journal at the close"
    );
    assert!(!workspace.quarantined("unblock.db-journal").exists());
}

// ---- regular files, confinement and modes (D58 amendment, tracker comment 388) ----

/// The unix permission bits of `path`.
fn mode(path: &Path) -> u32 {
    fs::metadata(path).expect("metadata").permissions().mode() & 0o777
}

/// Assert the window refused `not_a_regular_file` before `raw/` existed, copied nothing and wrote
/// no snapshot.
fn assert_refused_not_regular(workspace: &Workspace, outcome: &ExclusiveOutcome) {
    assert_eq!(outcome.refused, Some(ExclusiveRefusal::NotARegularFile));
    assert_eq!(
        ExclusiveRefusal::NotARegularFile.code(),
        "not_a_regular_file"
    );
    assert!(outcome.raw_copies.is_empty());
    assert_eq!(outcome.snapshot, None);
    assert!(outcome.renamed.is_empty());
    assert!(
        names(&workspace.run_dir).is_empty(),
        "no raw/ and no snapshot"
    );
}

/// A `-wal` that is a symlink to a file outside the workspace refuses `not_a_regular_file` before
/// any copy, and the link's target keeps its bytes (no TRUNCATE checkpoint reached it).
#[tokio::test]
async fn refuses_not_a_regular_file_for_a_symlinked_sidecar_and_copies_nothing() {
    let workspace = Workspace::migrated().await;
    let outside = tempfile::tempdir().expect("outside");
    let target = outside.path().join("victim");
    fs::write(&target, [1_u8, 2, 3, 4, 5]).expect("write the link target");
    symlink(&target, workspace.sidecar("-wal")).expect("symlink the -wal");

    let outcome = exclusive_window(&workspace.db, &workspace.request().checkpoint_truncate())
        .await
        .expect("window");

    assert_refused_not_regular(&workspace, &outcome);
    assert_eq!(fs::read(&target).expect("target"), [1_u8, 2, 3, 4, 5]);
}

/// A database file that is a symlink refuses `not_a_regular_file` before any copy.
#[tokio::test]
async fn refuses_not_a_regular_file_for_a_symlinked_database_and_copies_nothing() {
    let workspace = Workspace::migrated().await;
    let outside = tempfile::tempdir().expect("outside");
    let real = outside.path().join("unblock.db");
    fs::rename(&workspace.db, &real).expect("move the database out");
    symlink(&real, &workspace.db).expect("symlink the database");
    let bytes = fs::read(&real).expect("database bytes");

    let outcome = exclusive_window(&workspace.db, &workspace.request())
        .await
        .expect("window");

    assert_refused_not_regular(&workspace, &outcome);
    assert_eq!(fs::read(&real).expect("database"), bytes);
    assert_eq!(names(outside.path()), ["unblock.db"]);
}

/// Replace the workspace's `.recovery/` with a symlink to an outside directory holding an empty
/// run directory of the same name. Returns the outside directory.
fn link_recovery_outside(workspace: &Workspace) -> TempDir {
    let outside = tempfile::tempdir().expect("outside");
    let root = workspace.dir.path().join(".recovery");
    fs::remove_dir_all(&root).expect("remove .recovery");
    let run = workspace.run_dir.file_name().expect("run dir name");
    fs::create_dir(outside.path().join(run)).expect("outside run dir");
    symlink(outside.path(), &root).expect("symlink .recovery");
    outside
}

/// Assert `result` is `RecoveryPathOutsideWorkspace` naming `path`.
fn assert_outside<T: std::fmt::Debug>(result: Result<T, StorageError>, path: &Path) {
    let error = result.expect_err("refused");
    assert!(
        matches!(&error, StorageError::RecoveryPathOutsideWorkspace { path: refused } if refused == path),
        "got {error:?}"
    );
}

/// A symlinked `.recovery` refuses the window, and nothing is written through the link.
#[tokio::test]
async fn symlinked_recovery_refuses_the_window_and_writes_nothing_through_it() {
    let workspace = Workspace::migrated().await;
    let outside = link_recovery_outside(&workspace);

    assert_outside(
        exclusive_window(&workspace.db, &workspace.request()).await,
        &workspace.run_dir,
    );

    let run = workspace.run_dir.file_name().expect("run dir name");
    assert!(names(&outside.path().join(run)).is_empty());
}

/// A symlinked `.recovery` refuses `repair`, and `VACUUM INTO` writes nothing through the link.
#[tokio::test]
async fn symlinked_recovery_refuses_repair_and_writes_nothing_through_it() {
    let workspace = Workspace::migrated().await;
    let outside = link_recovery_outside(&workspace);
    let store = LibsqlStorage::open_local(&workspace.db, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open_local");
    let snapshot = workspace.run_dir.join("snapshot.db");

    assert_outside(
        store
            .repair(&RepairRequest {
                snapshot: snapshot.clone(),
                migrate: false,
                actions: Vec::new(),
            })
            .await,
        &snapshot,
    );

    let run = workspace.run_dir.file_name().expect("run dir name");
    assert!(names(&outside.path().join(run)).is_empty());
}

/// A run directory that is a symlink under a real `.recovery` refuses the window, and nothing is
/// written through it.
#[tokio::test]
async fn symlinked_run_dir_refuses_the_window_and_writes_nothing_through_it() {
    let workspace = Workspace::migrated().await;
    let outside = tempfile::tempdir().expect("outside");
    let linked = workspace.dir.path().join(".recovery").join("linked");
    symlink(outside.path(), &linked).expect("symlink the run dir");

    assert_outside(
        exclusive_window(&workspace.db, &ExclusiveRequest::new(linked.clone())).await,
        &linked,
    );

    assert!(names(outside.path()).is_empty());
}

/// The evidence is private on unix: `raw/` and `quarantine/` are 0700, the raw copies, the
/// renamed `-shm` and both snapshots (the window's and `repair`'s) are 0600, whatever the
/// database's own mode.
#[tokio::test]
async fn evidence_directories_are_0700_and_files_0600() {
    let workspace = Workspace::migrated().await;
    fs::set_permissions(&workspace.db, fs::Permissions::from_mode(0o644)).expect("chmod db");
    fs::write(workspace.sidecar("-shm"), pattern(32 * 1024)).expect("write a lone -shm");

    let outcome = exclusive_window(&workspace.db, &workspace.request().quarantine_shm())
        .await
        .expect("window");

    assert_eq!(outcome.refused, None);
    assert_eq!(mode(&workspace.run_dir.join("raw")), 0o700);
    assert_eq!(mode(&workspace.run_dir.join("quarantine")), 0o700);
    assert_eq!(mode(&workspace.raw("unblock.db")), 0o600);
    assert_eq!(mode(&workspace.raw("unblock.db-shm")), 0o600);
    assert_eq!(mode(&workspace.quarantined("unblock.db-shm")), 0o600);
    assert_eq!(
        mode(outcome.snapshot.as_deref().expect("window snapshot")),
        0o600
    );

    let store = LibsqlStorage::open_local(&workspace.db, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open_local");
    let run = workspace.dir.path().join(".recovery").join("online");
    fs::create_dir(&run).expect("online run dir");
    let repaired = store
        .repair(&RepairRequest {
            snapshot: run.join("snapshot.db"),
            migrate: false,
            actions: Vec::new(),
        })
        .await
        .expect("repair");
    assert_eq!(mode(&repaired.snapshot), 0o600);
}

/// A run directory outside `<db dir>/.recovery/` is an engine bug, refused before any copy.
#[tokio::test]
async fn run_dir_outside_recovery_is_refused_before_any_copy() {
    let workspace = Workspace::migrated().await;
    let outside = workspace.dir.path().join("elsewhere");
    fs::create_dir(&outside).expect("create");

    let error = exclusive_window(&workspace.db, &ExclusiveRequest::new(outside.clone()))
        .await
        .expect_err("refused");

    assert!(
        matches!(&error, StorageError::RecoveryPathOutsideWorkspace { path } if *path == outside),
        "got {error:?}"
    );
    assert!(names(&outside).is_empty());
}

// ---- the other three primitives ----

/// (clause 2) The probe never creates `.write.lock`, reads a free one as not held, and reads one a
/// second process holds as held.
#[tokio::test]
async fn probe_write_lock_never_creates_the_marker() {
    let workspace = Workspace::empty();
    assert!(!probe_write_lock(&workspace.db).expect("probe"));
    assert!(
        !workspace.marker().exists(),
        "an absent marker stays absent"
    );

    fs::write(workspace.marker(), b"").expect("create the marker");
    assert!(
        !probe_write_lock(&workspace.db).expect("probe"),
        "a free lock"
    );

    let holder = Holder::spawn("write-lock", &workspace.db);
    assert!(
        probe_write_lock(&workspace.db).expect("probe"),
        "held elsewhere"
    );
    drop(holder);
    assert!(!probe_write_lock(&workspace.db).expect("probe"), "released");
}

/// (clause 7) Absent database plus marker is LOST.
#[test]
fn lost_database_absent_with_marker_is_lost() {
    let workspace = Workspace::empty();
    fs::write(workspace.marker(), b"").expect("marker");
    assert!(lost_database(&workspace.db).expect("lost"));
    assert!(!workspace.db.exists(), "the predicate creates nothing");
}

/// (clause 7) A 0-byte database plus marker is LOST.
#[test]
fn lost_database_empty_with_marker_is_lost() {
    let workspace = Workspace::empty();
    fs::write(workspace.marker(), b"").expect("marker");
    fs::write(&workspace.db, b"").expect("0-byte database");
    assert!(lost_database(&workspace.db).expect("lost"));
}

/// (clause 7) Without the marker an absent or 0-byte database is a fresh clone, never lost.
#[test]
fn lost_database_without_marker_is_not_lost() {
    let workspace = Workspace::empty();
    assert!(!lost_database(&workspace.db).expect("absent"));
    fs::write(&workspace.db, b"").expect("0-byte database");
    assert!(!lost_database(&workspace.db).expect("empty"));
    assert!(
        !workspace.marker().exists(),
        "the predicate creates nothing"
    );
}

/// (clause 7) A database with content is never lost, marker or not.
#[tokio::test]
async fn lost_database_with_content_is_not_lost() {
    let workspace = Workspace::migrated().await;
    assert!(!lost_database(&workspace.db).expect("lost"));
}

/// (clause 5) The registry counts each file-backed connection per database file and forgets it on
/// close. An absent file has none.
#[tokio::test]
async fn live_connections_to_counts_per_file() {
    let first = Workspace::migrated().await;
    let second = Workspace::migrated().await;
    assert_eq!(live_connections_to(&first.db).expect("live"), 0);
    assert_eq!(
        live_connections_to(&first.dir.path().join("absent.db")).expect("live"),
        0
    );

    let first_store = LibsqlStorage::open_local(&first.db, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open first");
    assert_eq!(live_connections_to(&first.db).expect("live"), 2);
    assert_eq!(live_connections_to(&second.db).expect("live"), 0);

    let second_store = LibsqlStorage::open_local(&second.db, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open second");
    assert_eq!(live_connections_to(&second.db).expect("live"), 2);

    let in_memory = LibsqlStorage::open_in_memory().await.expect("in-memory");
    assert_eq!(live_connections_to(&first.db).expect("live"), 2);

    drop(first_store);
    assert_eq!(live_connections_to(&first.db).expect("live"), 0);
    assert_eq!(live_connections_to(&second.db).expect("live"), 2);
    drop(second_store);
    drop(in_memory);
    assert_eq!(live_connections_to(&second.db).expect("live"), 0);
}

// ---- foreign descriptor on the database file (ub-2vp) ----

/// What one observer process O printed.
struct Observed {
    line: String,
    stderr: String,
}

impl Observed {
    /// `BUSY <code>` with `code & 0xff == SQLITE_BUSY`.
    fn is_busy(&self) -> bool {
        self.line
            .strip_prefix("BUSY ")
            .and_then(|code| code.parse::<i32>().ok())
            .is_some_and(|code| code & 0xff == 5)
    }

    fn is_locked(&self) -> bool {
        self.line == "LOCKED"
    }
}

/// Run a fresh observer O on `db` to completion. It tries `locking_mode=EXCLUSIVE` once.
fn observe(db: &Path) -> Observed {
    let output = Command::new(HOLDER)
        .arg("observe-exclusive")
        .arg(db)
        .output()
        .expect("run the observer");
    let line = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "the observer exits 0 (line {line:?}, stderr {stderr})"
    );
    Observed { line, stderr }
}

/// Run a one-shot ordinary process B on `db` to completion. It opens the database, reads once and
/// closes.
fn read_and_close(db: &Path) {
    let output = Command::new(HOLDER)
        .arg("read-and-close")
        .arg(db)
        .output()
        .expect("run read-and-close");
    assert!(
        output.status.success(),
        "read-and-close exits 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "CLOSED");
}

/// Whether `unblock.db-wal` and `unblock.db-shm` exist.
fn sidecars(workspace: &Workspace) -> (bool, bool) {
    (
        workspace.sidecar("-wal").exists(),
        workspace.sidecar("-shm").exists(),
    )
}

/// The holder's `LIVE` reply as a count.
fn live(holder: &mut Holder) -> usize {
    let reply = holder.send("LIVE");
    reply
        .strip_prefix("LIVE ")
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("unexpected LIVE reply {reply:?}"))
}

/// Print the `UB2VP-MEASURE` line of one observation and return the observation.
fn measure(
    cell: &str,
    point: &str,
    holder_pid: i32,
    observation: Observation,
    observer: Option<&Observed>,
    live: Option<usize>,
) -> Observation {
    eprintln!(
        "{}",
        posix_locks::measure_line(
            cell,
            point,
            holder_pid,
            &observation,
            observer.map(|observed| observed.line.as_str()),
            live,
        )
    );
    observation
}

/// Print one `UB2VP-MEASURE` line recording a protocol reply or a file state.
fn record(cell: &str, point: &str, what: &str) {
    eprintln!(
        "UB2VP-MEASURE leg={} cell={cell} point={point} {what}",
        posix_locks::leg()
    );
}

/// The advice a red fd1, fd5 or fd6 prints. Each of those cells pins a measured result, and the
/// two guards it names exist because of that result.
fn red_advice(cell: &str) -> String {
    format!(
        "a red {cell} is a MEASUREMENT, so re-measure and amend PRD §4 D58; never remove the two \
         live-connection guards, which are gather_preopen's `live > 0` refusal \
         (`PreOpenWithLiveConnection`, crates/unblock-engine/src/preopen.rs) and step (a2) of \
         `run_window` in crates/unblock-storage/src/libsql/health.rs \
         (`ExclusiveRefusal::ProcessHoldsConnection`)"
    )
}

/// A migrated workspace and a `lock-probe` holder A whose SHARED lock is asserted, not assumed.
async fn probe_holder(cell: &str) -> (Workspace, Holder) {
    let workspace = Workspace::migrated().await;
    assert_eq!(live_connections_to(&workspace.db).expect("live"), 0);
    let mut holder = Holder::spawn("lock-probe", &workspace.db);
    let pid = holder.pid();
    let live = live(&mut holder);
    let baseline = measure(
        cell,
        "baseline",
        pid,
        posix_locks::shared_range(&workspace.db, pid),
        None,
        Some(live),
    );
    assert_eq!(
        baseline.kind(),
        Some(LockKind::Read),
        "{cell}: the holder's store holds SQLite's SHARED lock before the step"
    );
    assert_eq!(live, 2, "{cell}: the holder's store has two connections");
    let version = holder.send("VERSION");
    record(cell, "version", &format!("reply={version:?}"));
    (workspace, holder)
}

/// The SHARED observation of `holder` after a step, with the observer O run last.
fn shared_then_observer(
    cell: &str,
    point: &str,
    workspace: &Workspace,
    holder: &mut Holder,
) -> (Observation, Observed) {
    let pid = holder.pid();
    let live = live(holder);
    let observation = posix_locks::shared_range(&workspace.db, pid);
    let observed = observe(&workspace.db);
    let observation = measure(cell, point, pid, observation, Some(&observed), Some(live));
    assert_eq!(live, 2, "{cell}: LIVE 2 at {point}");
    (observation, observed)
}

/// (n, fd0) The holder's store holds `SQLite`'s SHARED lock, and the observer detects it. This
/// cell checks the detector and the precondition of every other fd cell.
#[tokio::test]
async fn fd0_store_holds_the_shared_lock() {
    let (workspace, mut holder) = probe_holder("fd0").await;
    let (observation, observed) = shared_then_observer("fd0", "held", &workspace, &mut holder);
    assert_eq!(observation.kind(), Some(LockKind::Read));
    assert!(
        observed.is_busy(),
        "the observer must be BUSY beside a live store, got {:?} (stderr {})",
        observed.line,
        observed.stderr
    );
}

/// (n, fd1) THE REPRODUCTION. The v1.0.x header probe, copied verbatim and run in a process holding
/// libsql WAL connections, drops that process's SHARED lock, and neither a later read nor a
/// SQLite-owned close takes it back. Asserted in the MEASURED direction (PRD §4 D58 criterion (n),
/// measured 2026-11-18 on Linux and macOS, tracker comment 434).
#[tokio::test]
async fn fd1_v1_header_probe_close_drops_the_shared_lock() {
    let (workspace, mut holder) = probe_holder("fd1").await;
    let pid = holder.pid();

    let probed = holder.send("PROBE-V1");
    record("fd1", "probe", &format!("reply={probed:?}"));
    let live_probe = live(&mut holder);
    let after_probe = measure(
        "fd1",
        "after-probe",
        pid,
        posix_locks::shared_range(&workspace.db, pid),
        None,
        Some(live_probe),
    );
    let read = holder.send("READ");
    record("fd1", "read", &format!("reply={read:?}"));
    let live_read = live(&mut holder);
    let after_read = measure(
        "fd1",
        "after-read",
        pid,
        posix_locks::shared_range(&workspace.db, pid),
        None,
        Some(live_read),
    );
    let reopened = holder.send("SQLITE-REOPEN");
    let (after_reopen, observed) =
        shared_then_observer("fd1", "after-sqlite-reopen", &workspace, &mut holder);

    let advice = red_advice("fd1");
    assert_eq!(
        probed, "PROBED mismatch=false",
        "the probe reads a valid header"
    );
    assert!(
        read.starts_with("READ "),
        "the store still reads, got {read:?}"
    );
    assert_eq!(reopened, "REOPENED");
    assert_eq!(live_probe, 2, "LIVE 2 after the probe");
    assert_eq!(live_read, 2, "LIVE 2 after the read");
    assert_eq!(
        after_probe.kind(),
        None,
        "the probe's close drops the SHARED lock; {advice}"
    );
    assert_eq!(
        after_read.kind(),
        None,
        "a later read does not retake the SHARED lock; {advice}"
    );
    assert_eq!(
        after_reopen.kind(),
        None,
        "a SQLite-owned reopen does not retake the SHARED lock; {advice}"
    );
    assert!(
        observed.is_locked(),
        "the observer takes the database the holder no longer guards, got {:?} (stderr {}); \
         {advice}",
        observed.line,
        observed.stderr
    );
}

/// (n, fd2) The no-op control shows that a round trip of the protocol drops nothing.
#[tokio::test]
async fn fd2_control_noop_keeps_the_shared_lock() {
    let (workspace, mut holder) = probe_holder("fd2").await;
    assert_eq!(holder.send("NOOP"), "OK");
    let (observation, observed) =
        shared_then_observer("fd2", "after-noop", &workspace, &mut holder);
    assert_eq!(observation.kind(), Some(LockKind::Read));
    assert!(
        observed.is_busy(),
        "got {:?} (stderr {})",
        observed.line,
        observed.stderr
    );
}

/// (n, fd3) In this control, closing descriptors `SQLite` itself opened keeps the lock, because
/// `setPendingFd` defers their close. The hazard is specific to foreign descriptors.
#[tokio::test]
async fn fd3_control_sqlite_owned_close_keeps_the_shared_lock() {
    let (workspace, mut holder) = probe_holder("fd3").await;
    assert_eq!(holder.send("SQLITE-REOPEN"), "REOPENED");
    let (observation, observed) =
        shared_then_observer("fd3", "after-sqlite-reopen", &workspace, &mut holder);
    assert_eq!(observation.kind(), Some(LockKind::Read));
    assert!(
        observed.is_busy(),
        "got {:?} (stderr {})",
        observed.line,
        observed.stderr
    );
}

/// (n, fd4 = G1) Main's pre-open primitives (`lost_database`, `live_connections_to`,
/// `probe_write_lock`) open no descriptor on the database file, so a live store keeps its lock.
#[tokio::test]
async fn fd4_preopen_primitives_keep_the_shared_lock() {
    let (workspace, mut holder) = probe_holder("fd4").await;
    let primitives = holder.send("PRIMITIVES");
    record("fd4", "primitives", &format!("reply={primitives:?}"));
    assert_eq!(primitives, "PRIMITIVES lost=false live=2 write_lock=false");
    let (observation, observed) =
        shared_then_observer("fd4", "after-primitives", &workspace, &mut holder);
    assert_eq!(
        observation.kind(),
        Some(LockKind::Read),
        "a pre-open primitive opened the database file and dropped the store's lock"
    );
    assert!(
        observed.is_busy(),
        "got {:?} (stderr {})",
        observed.line,
        observed.stderr
    );
}

/// Build the fd5 state, in which a probed holder A sees an ordinary process B run to completion.
/// Returns the workspace, A, and the sidecars' presence after B.
async fn probed_then_closed_elsewhere(cell: &str) -> (Workspace, Holder, (bool, bool)) {
    let (workspace, mut holder) = probe_holder(cell).await;
    assert_eq!(
        sidecars(&workspace),
        (true, true),
        "{cell}: the holder's WAL open has a -wal and a -shm"
    );
    let probed = holder.send("PROBE-V1");
    record(cell, "probe", &format!("reply={probed:?}"));
    assert_eq!(probed, "PROBED mismatch=false");
    read_and_close(&workspace.db);
    let after = sidecars(&workspace);
    record(cell, "after-b", &format!("wal={} shm={}", after.0, after.1));
    (workspace, holder, after)
}

/// (n, fd5) The consequence. After the probe, another process's last close takes EXCLUSIVE,
/// checkpoints and unlinks the `-wal` and `-shm` while the holder is alive and still has them open.
/// Asserted in the MEASURED direction (PRD §4 D58 criterion (n), measured 2026-11-18).
#[tokio::test]
async fn fd5_last_close_elsewhere_unlinks_the_live_sidecars_after_the_probe() {
    let (_workspace, mut holder, after) = probed_then_closed_elsewhere("fd5").await;
    let alive = holder.child.try_wait().expect("try_wait").is_none();
    let live = live(&mut holder);
    let read = holder.send("READ");
    record(
        "fd5",
        "holder-after-b",
        &format!("alive={alive} live={live} reply={read:?}"),
    );
    assert!(alive, "the holder is alive");
    assert_eq!(live, 2, "the holder's store still has two connections");
    assert!(
        read.starts_with("READ "),
        "the holder still reads after the unlink, got {read:?}"
    );
    assert_eq!(
        after,
        (false, false),
        "another process's last close unlinks the -wal and -shm the holder still has open; {}",
        red_advice("fd5")
    );
}

/// (n, fd5c) In this control there is no probe, so the holder's SHARED lock blocks B's EXCLUSIVE
/// and B's close keeps the sidecars.
#[tokio::test]
async fn fd5c_control_last_close_elsewhere_keeps_the_sidecars_without_the_probe() {
    let (workspace, mut holder) = probe_holder("fd5c").await;
    read_and_close(&workspace.db);
    let present = sidecars(&workspace);
    record(
        "fd5c",
        "after-b",
        &format!("wal={} shm={}", present.0, present.1),
    );
    let pid = holder.pid();
    let live = live(&mut holder);
    let observation = measure(
        "fd5c",
        "after-b",
        pid,
        posix_locks::shared_range(&workspace.db, pid),
        None,
        Some(live),
    );
    assert_eq!(present, (true, true), "the sidecars stay");
    assert_eq!(observation.kind(), Some(LockKind::Read));
}

/// Open the workspace's database in this process, record each id's presence and the
/// `integrity_check` rows, and return both.
async fn final_state(cell: &str, workspace: &Workspace, ids: &[&str]) -> (Vec<bool>, Vec<String>) {
    let store = LibsqlStorage::open_local(&workspace.db, DEFAULT_WRITE_LOCK_TIMEOUT_MS)
        .await
        .expect("open_local");
    let mut found = Vec::new();
    for id in ids {
        found.push(store.get_issue(id).await.expect("get_issue").is_some());
    }
    let integrity = store.integrity_check().await.expect("integrity_check");
    record(
        cell,
        "final",
        &format!("ids={ids:?} get={found:?} integrity={integrity:?}"),
    );
    drop(store);
    (found, integrity)
}

/// (n, fd6) The worst case for a writing orphan, MEASURED 2026-11-18 (tracker comment 434) and
/// pinned. After the unlink the holder's commit succeeds, a second holder A2 that holds the NEW
/// files never sees it, before or after the first holder quits, and the row is LOST with a clean
/// `integrity_check`, so the loss is silent. A2's own control write is FOUND, so the protocol and
/// the store work. The cell is the rationale for main's live-connection guards before a repair.
#[tokio::test]
async fn fd6_commit_after_unlink_is_lost_while_another_process_holds_the_new_files() {
    let (workspace, mut holder, _) = probed_then_closed_elsewhere("fd6").await;
    let wrote = holder.send("WRITE ub2vp-fd6");
    record("fd6", "a-write", &format!("reply={wrote:?}"));
    let mut second = Holder::spawn("lock-probe", &workspace.db);
    let a2_first = second.send("GET ub2vp-fd6");
    record(
        "fd6",
        "a2-get-before-a-quits",
        &format!("reply={a2_first:?}"),
    );
    let bye = holder.send("QUIT");
    let holder_status = holder.quit();
    record(
        "fd6",
        "a-quit",
        &format!("reply={bye:?} status={holder_status}"),
    );
    let a2_second = second.send("GET ub2vp-fd6");
    record(
        "fd6",
        "a2-get-after-a-quits",
        &format!("reply={a2_second:?}"),
    );
    let control_wrote = second.send("WRITE ub2vp-fd6-control");
    let control_got = second.send("GET ub2vp-fd6-control");
    record(
        "fd6",
        "a2-control",
        &format!("write={control_wrote:?} get={control_got:?}"),
    );
    let second_bye = second.send("QUIT");
    let second_status = second.quit();
    let (found, integrity) =
        final_state("fd6", &workspace, &["ub2vp-fd6", "ub2vp-fd6-control"]).await;

    let advice = red_advice("fd6");
    assert_eq!(
        wrote, "WROTE ub2vp-fd6",
        "the orphaned holder's commit succeeds"
    );
    assert_eq!(bye, "BYE");
    assert_eq!(second_bye, "BYE");
    assert!(
        holder_status.success(),
        "the orphaned holder exits 0, got {holder_status}"
    );
    assert!(
        second_status.success(),
        "the second holder exits 0, got {second_status}"
    );
    assert_eq!(
        control_wrote, "WROTE ub2vp-fd6-control",
        "the second holder's control commit succeeds"
    );
    assert_eq!(
        control_got, "FOUND",
        "the second holder reads its own control row, so WRITE and GET work"
    );
    assert_eq!(
        a2_first, "ABSENT",
        "the second holder never sees the orphan's commit; {advice}"
    );
    assert_eq!(a2_second, "ABSENT", "nor after the orphan quits; {advice}");
    assert_eq!(
        found,
        [false, true],
        "the orphan's row is lost and the control row is present; {advice}"
    );
    assert!(
        integrity.is_empty(),
        "the loss is silent because integrity_check is clean, got {integrity:?}; {advice}"
    );
}

/// (n/(c), fd7) The real exclusive window, observed inside step (c). At P1 (after
/// `locking_mode=EXCLUSIVE` holds, before the evidence check opens the sidecars) the window holds
/// Write on the SHARED range, nothing locks the `-wal` or the `-shm`, and the observer is BUSY. At
/// P2 (after the evidence check) the window still holds Write. The combined lone `-shm` + 5-byte
/// `-wal` setup reaches P2 and completes (`refused=None checkpoint_truncated=true renamed=1`,
/// measured 2026-11-18 on every leg, tracker comment 434).
#[tokio::test]
async fn fd7_window_sidecar_reread_keeps_its_exclusive_lock() {
    let workspace = Workspace::migrated().await;
    let wal = workspace.sidecar("-wal");
    let shm = workspace.sidecar("-shm");
    fs::write(&shm, pattern(32 * 1024)).expect("write a lone -shm");
    fs::write(&wal, [1_u8, 2, 3, 4, 5]).expect("write a truncated -wal");
    assert!(
        workspace.run_dir.is_dir(),
        "the run directory is under .recovery/"
    );
    let mut window = Holder::spawn_with("window", &workspace.db, &[&workspace.run_dir]);
    let pid = window.pid();

    // P1 sits after step (c)'s lock and before the evidence check.
    let held = window.send("GO");
    record("fd7", "p1", &format!("reply={held:?}"));
    assert_eq!(held, "EXCLUSIVE-HELD", "the window reached P1");
    assert!(wal.exists() && shm.exists(), "both sidecars exist at P1");
    let p1_shared = posix_locks::shared_range(&workspace.db, pid);
    let p1_wal = measure(
        "fd7",
        "p1-wal",
        pid,
        posix_locks::whole_file(&wal),
        None,
        None,
    );
    let p1_shm = measure(
        "fd7",
        "p1-shm",
        pid,
        posix_locks::whole_file(&shm),
        None,
        None,
    );
    let p1_observed = observe(&workspace.db);
    let p1_shared = measure("fd7", "p1", pid, p1_shared, Some(&p1_observed), None);
    assert_eq!(
        p1_shared.kind(),
        Some(LockKind::Write),
        "the window holds EXCLUSIVE"
    );
    for (name, observation) in [("-wal", &p1_wal), ("-shm", &p1_shm)] {
        assert_eq!(
            observation.kind(),
            None,
            "the window holds a lock on the {name}, so it likely mapped the -shm (DMS lock) or \
             reopened a sidecar before `locking_mode=EXCLUSIVE` (`connect_raw`/`lock_exclusively` \
             order)"
        );
        assert!(
            observation
                .proc_locks
                .as_ref()
                .is_none_or(std::vec::Vec::is_empty),
            "/proc/locks has no line for the {name}, got {:?}",
            observation.proc_locks
        );
    }
    assert!(
        p1_observed.is_busy(),
        "the observer is BUSY at P1, got {:?} (stderr {})",
        p1_observed.line,
        p1_observed.stderr
    );

    // P2 sits after the evidence check, which passes on the combined setup.
    let checked = window.send("NEXT");
    record("fd7", "p2", &format!("reply={checked:?}"));
    assert_eq!(
        checked, "EVIDENCE-CHECKED",
        "the evidence check passes on the combined setup and the window reaches P2"
    );
    let p2_shared = posix_locks::shared_range(&workspace.db, pid);
    let p2_observed = observe(&workspace.db);
    let p2_shared = measure("fd7", "p2", pid, p2_shared, Some(&p2_observed), None);
    assert_eq!(
        p2_shared.kind(),
        Some(LockKind::Write),
        "the evidence check kept the window's EXCLUSIVE lock"
    );
    assert!(
        p2_observed.is_busy(),
        "the observer is BUSY at P2, got {:?} (stderr {})",
        p2_observed.line,
        p2_observed.stderr
    );

    let outcome = window.send("NEXT");
    record("fd7", "outcome", &format!("reply={outcome:?}"));
    assert_eq!(
        outcome, "OUTCOME refused=None checkpoint_truncated=true renamed=1",
        "the combined lone -shm + 5-byte -wal setup completes, so the -shm is quarantined and \
         the TRUNCATE checkpoint runs"
    );
    assert!(window.quit().success());
}

/// (n, fdp) The storage `/proc/locks` parser reads canned lines. It keeps the POSIX lines of one
/// inode and pid, decodes the hex device, skips a blocked waiter's `->` line and reads `EOF` as
/// length 0. A line on the inode with another device is excluded when some line carries the
/// `stat` device, and kept when none does (the btrfs fallback).
#[cfg(target_os = "linux")]
#[test]
fn fd_proc_locks_parser_reads_canned_lines() {
    use posix_locks::{HeldLock, dev_major_minor, parse_proc_locks, raw_lines_on_inode};

    let text = "\
1: POSIX  ADVISORY  READ  4242 08:01:123456 1073741826 1073742335
1: -> POSIX  ADVISORY  WRITE 4343 08:01:123456 1073741826 1073742335
2: POSIX  ADVISORY  WRITE 4242 fd:00:999 0 EOF
3: FLOCK  ADVISORY  WRITE 5151 08:01:777 0 EOF
4: POSIX  ADVISORY  WRITE 4242 08:01:123456 128 255
5: POSIX  ADVISORY  READ  4242 00:2a:123456 0 EOF
6: POSIX  ADVISORY  READ  5353 00:2c:4040 1073741826 1073742335
7: POSIX  ADVISORY  READ  5353 00:2c:4040 1073741826 1073742335
";
    assert_eq!(
        parse_proc_locks(text, (8, 1), 123_456, Some(4242)),
        [
            HeldLock {
                kind: LockKind::Read,
                pid: 4242,
                start: 1_073_741_826,
                len: 510,
                dev: Some("08:01".to_owned()),
            },
            HeldLock {
                kind: LockKind::Write,
                pid: 4242,
                start: 128,
                len: 128,
                dev: Some("08:01".to_owned()),
            },
        ],
        "a line carries the stat device 08:01, so the same inode on device 00:2a is excluded"
    );
    assert_eq!(
        parse_proc_locks(text, (0xfd, 0), 999, None),
        [HeldLock {
            kind: LockKind::Write,
            pid: 4242,
            start: 0,
            len: 0,
            dev: Some("fd:00".to_owned()),
        }],
        "the device is read as hex"
    );
    assert_eq!(
        parse_proc_locks(text, (0, 0x2a), 123_456, None),
        [HeldLock {
            kind: LockKind::Read,
            pid: 4242,
            start: 0,
            len: 0,
            dev: Some("00:2a".to_owned()),
        }],
        "a line carries the stat device 00:2a, so the lines on device 08:01 are excluded"
    );
    assert_eq!(
        parse_proc_locks(text, (0, 0x3a), 4040, Some(5353)),
        [HeldLock {
            kind: LockKind::Read,
            pid: 5353,
            start: 1_073_741_826,
            len: 510,
            dev: Some("00:2c".to_owned()),
        }],
        "no line carries the subvolume device 00:3a (btrfs), so the inode alone matches"
    );
    assert_eq!(
        parse_proc_locks(text, (8, 2), 123_456, None).len(),
        3,
        "no line carries device 08:02, so every line on the inode matches"
    );
    assert_eq!(
        parse_proc_locks(text, (0, 0x3a), 4040, None).len(),
        1,
        "a torn read that repeats a line yields one record"
    );
    assert_eq!(
        raw_lines_on_inode(text, 4040).len(),
        2,
        "the failure evidence keeps every raw line on the inode"
    );
    assert_eq!(
        raw_lines_on_inode(text, 123_456).len(),
        4,
        "the failure evidence keeps waiters and every device"
    );
    assert!(parse_proc_locks(text, (8, 1), 123_456, Some(1)).is_empty());
    assert!(parse_proc_locks(text, (8, 1), 31_337, None).is_empty());
    assert_eq!(dev_major_minor(0x0801), (8, 1));
    assert_eq!(dev_major_minor(0x1001_0305), (0x103, 0x1_0005));
}
