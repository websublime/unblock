//! PRD §4 D58 criterion (f), MEASURED, plus the storage halves of criteria (g), (h) and (n) and the
//! three other local-file primitives (`lost_database`, `live_connections_to`, `probe_write_lock`).
//!
//! The `f*` cells measure the exclusivity proof against a REAL second process, the
//! `exclusive_window_holder` helper `[[bin]]`. It holds a libsql reader that has served one read,
//! the `.write.lock`, or a killed write transaction's hot `-journal`, and it runs the (f5) control.
//! The cells run on Linux in `storage-testkit` and on macOS in `exclusivity-macos`. Windows takes
//! another VFS locking path and stays unmeasured, so the file is Unix-only. The `f6` cells need the
//! testkit-only `with_after_raw_copies` hook.
#![cfg(all(unix, feature = "testkit"))]

use std::fs;
use std::io::{BufRead as _, BufReader};
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

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

/// A running `exclusive_window_holder`. Dropping it kills the process and reaps it.
struct Holder {
    child: Child,
    _stdin: Option<ChildStdin>,
}

impl Holder {
    /// Spawn `mode` on `db` and wait for its `READY` line.
    fn spawn(mode: &str, db: &Path) -> Self {
        let mut child = Command::new(HOLDER)
            .arg(mode)
            .arg(db)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn exclusive_window_holder");
        let stdout = child.stdout.take().expect("holder stdout");
        let mut line = String::new();
        BufReader::new(stdout)
            .read_line(&mut line)
            .expect("read the holder's stdout");
        assert_eq!(
            line.trim_end(),
            "READY",
            "the {mode} holder never got ready"
        );
        let stdin = child.stdin.take();
        Self {
            child,
            _stdin: stdin,
        }
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
