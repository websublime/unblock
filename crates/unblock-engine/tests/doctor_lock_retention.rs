//! PRD §4 D29 clause F6 (`ub-kpe`, v1.0.3): `unblock doctor` keeps the session's `SQLite` locks.
//!
//! From v1.0.0 to v1.0.2 the doctor's file-state check opened `unblock.db` with `std::fs`, read 16
//! bytes and closed the descriptor inside the live session; on Linux and macOS that close releases
//! every POSIX lock the process holds on the file (sqlite.org `howtocorrupt.html` §2.2), so the
//! session lost its SHARED lock (measured under `ub-2vp`, tracker comments 434 and 435).
//!
//! A HOLDER process (`doctor_lock_holder`) opens a `Session` in the cli's doctor order, reads, and runs
//! the real `Session::doctor(preflight)` on command. The parent asserts, from outside, that the holder
//! still holds SHARED (`F_GETLK`, plus `/proc/locks` on Linux) and that a FRESH observer process (this
//! test binary re-executed at `kpe_observer_entry`, its token parsed anywhere in stdout, `-q`) running
//! `PRAGMA locking_mode=EXCLUSIVE` + `BEGIN EXCLUSIVE` with `busy_timeout = 0` gets `SQLITE_BUSY`.
//! `kpe0` is the control without doctor; `kpe1m` and `kpe6m` are in-tree positive controls proving the
//! instrument can SEE a lock loss. Each observation prints one `UB2VP-MEASURE` evidence line.
#![cfg(all(unix, feature = "health"))]

mod common;

use std::path::Path;

use common::lock_harness::{
    HEALTHY_LABELS, Holder, Workspace, assert_shared_and_busy, assert_shm_dms_held, measure,
    observe,
};
use common::posix_locks;
use unblock_config::{
    CliOverrides, ConfigError, open_resolved_with_storage, open_with_storage_with_cli,
    open_workspace_with_cli,
};
use unblock_engine::{DoctorPreflight, Session, SessionConfig};
use unblock_error::CodedError as _;
use unblock_health::{DbHeader, DbHeaderProbe};

/// This binary's observer entry (see `common::lock_observer`).
const ENTRY: &str = "kpe_observer_entry";

/// The failure message of the fix cells: the mutant a red cell means.
const DOCTOR_MUTANT: &str =
    "a descriptor on unblock.db opened and closed after the session opened (D29 clause F6)";

/// The failure message of the same-file cells.
const SAME_FILE_MUTANT: &str = "the JSONL conflict scan opened the db inode inside the session (D29 clause F6, same-file guard)";

/// The observer process body. Spawned by the cells as `current_exe() kpe_observer_entry --exact
/// --include-ignored --nocapture --test-threads=1 -q` with `UBKPE_OBSERVE_DB=<db>`; a no-op otherwise.
#[test]
#[ignore = "observer process entry point; spawned by the kpe cells"]
fn kpe_observer_entry() {
    common::lock_observer::observer_main();
}

/// Spawn the holder on `ws` and assert the baseline: SHARED held and a fresh observer BUSY.
fn ready_holder(cell: &str, ws: &Workspace) -> Holder {
    let holder = Holder::spawn(ws);
    assert_shared_and_busy(
        ENTRY,
        cell,
        "baseline",
        &ws.db,
        &holder,
        "the holder must hold SHARED before the step",
    );
    holder
}

/// `DOCTOR` then the two post-doctor observations (`kpe1`'s shape), returning the `DOCTORED` reply.
fn doctor_and_observe(cell: &str, ws: &Workspace, holder: &mut Holder, why: &str) -> String {
    let doctored = holder.send("DOCTOR");
    assert!(
        doctored.starts_with("DOCTORED "),
        "{cell}: {doctored:?} (holder stderr {})",
        holder.stderr()
    );
    assert_shared_and_busy(ENTRY, cell, "after-doctor", &ws.db, holder, why);
    let read = holder.send("READ");
    assert!(read.starts_with("READ "), "{cell}: {read:?}");
    assert_shared_and_busy(ENTRY, cell, "after-read", &ws.db, holder, why);
    doctored
}

/// (kpe0) CONTROL, no doctor: the holder holds SHARED and the observer is BUSY across a no-op.
#[tokio::test]
async fn kpe0_control_no_doctor_keeps_the_shared_lock() {
    let ws = Workspace::migrated().await;
    let mut holder = ready_holder("kpe0", &ws);
    assert_eq!(holder.send("NOOP"), "OK");
    assert_shared_and_busy(ENTRY, "kpe0", "after-noop", &ws.db, &holder, "control");
    holder.quit();
}

/// (kpe1) THE FIX CELL: the real `Session::doctor(preflight)` keeps SHARED, also after a later read.
/// FAILS on unpatched v1.0.2 (non-vacuity, run once out of tree).
#[tokio::test]
async fn kpe1_doctor_keeps_the_shared_lock() {
    let ws = Workspace::migrated().await;
    let mut holder = ready_holder("kpe1", &ws);
    let doctored = doctor_and_observe("kpe1", &ws, &mut holder, DOCTOR_MUTANT);
    assert_eq!(doctored, HEALTHY_LABELS, "no database_not_sqlite finding");
    assert_eq!(holder.send("DOCTOR"), "DOCTOR-SPENT");
    holder.quit();
}

/// (kpe1m) IN-TREE POSITIVE CONTROL (the `ub-2vp` fd1 shape): the holder runs the deleted v1.0.2 probe
/// verbatim, and the instrument must see SHARED lost and the observer `LOCKED`. Proves on every run, on
/// both OSes, that a green `kpe1` is not vacuous.
#[tokio::test]
async fn kpe1m_v102_probe_drops_the_shared_lock() {
    let ws = Workspace::migrated().await;
    let mut holder = ready_holder("kpe1m", &ws);
    assert_eq!(holder.send("PROBE-V102"), "PROBED false");
    let pid = holder.pid();
    let observation = posix_locks::shared_range(&ws.db, pid);
    let observed = observe(ENTRY, &ws.db);
    let observation = measure(
        "kpe1m",
        "after-v102-probe",
        pid,
        observation,
        Some(&observed),
    );
    assert_eq!(
        observation.kind(),
        None,
        "the v1.0.2 probe's close must drop SHARED (the instrument sees a loss)"
    );
    if let Some(lines) = &observation.proc_locks {
        assert!(lines.is_empty(), "/proc/locks agrees: no lock ({lines:?})");
    }
    assert!(
        observed.is_locked(),
        "the observer must take the database EXCLUSIVE, got {:?} (stderr {})",
        observed.line,
        observed.stderr
    );
}

/// The CLI flow in process over `dir` (resolve → capture → open → `Session::open` → doctor).
async fn cli_flow_report(dir: &Path) -> unblock_model::DiagnosticReport {
    let resolved = open_workspace_with_cli(&CliOverrides::new().with_dir(dir))
        .await
        .expect("resolve");
    let preflight = DoctorPreflight::capture(&resolved.paths.db_path);
    let ctx = open_resolved_with_storage(resolved).await.expect("open");
    let session = Session::open(
        ctx,
        SessionConfig {
            import_on_open: false,
            ..SessionConfig::default()
        },
    )
    .await
    .expect("session");
    let report = session.doctor(preflight).await.expect("doctor");
    drop(session);
    report
}

fn bare_workspace() -> (tempfile::TempDir, std::path::PathBuf) {
    let root = tempfile::tempdir().expect("tempdir");
    let unblock = root.path().join(".unblock");
    std::fs::create_dir(&unblock).expect("mkdir .unblock");
    (root, unblock)
}

/// (kpe2) K1b: a 0-byte database is NOT a header mismatch, and behaves exactly as an absent one.
#[tokio::test]
async fn kpe2_zero_byte_db_is_not_a_header_mismatch() {
    let (_empty_root, empty) = bare_workspace();
    let db = empty.join("unblock.db");
    std::fs::write(&db, b"").expect("0-byte db");
    assert_eq!(DbHeaderProbe::read(&db).header(), DbHeader::Empty);
    let report = cli_flow_report(&empty).await;
    let labels: Vec<&str> = report.findings.iter().map(|f| f.label.as_str()).collect();
    assert_eq!(
        labels,
        ["health", "integrity", "schema_version", "schema_expected"]
    );
    let health = report
        .findings
        .iter()
        .find(|f| f.label == "health")
        .expect("health finding");
    assert_eq!(health.detail, "healthy");

    let (_absent_root, absent) = bare_workspace();
    let absent_report = cli_flow_report(&absent).await;
    assert_eq!(
        serde_json::to_value(&report).expect("json"),
        serde_json::to_value(&absent_report).expect("json"),
        "a 0-byte database reports exactly as an absent one (K1b)"
    );
}

/// (kpe3) A non-SQLite file still yields v1.0.2's outcome: the open fails identically through the old
/// facade and the new resolve-then-open path, and the file is untouched.
#[tokio::test]
async fn kpe3_non_sqlite_db_keeps_todays_open_failure() {
    for payload in [b"short".to_vec(), vec![b'x'; 64]] {
        let (_root, unblock) = bare_workspace();
        let db = unblock.join("unblock.db");
        std::fs::write(&db, &payload).expect("write payload");
        assert_eq!(DbHeaderProbe::read(&db).header(), DbHeader::NotSqlite);
        let overrides = CliOverrides::new().with_dir(&unblock);
        let Err(e1) = open_with_storage_with_cli(&overrides).await else {
            panic!("the v1.0.2 path must fail on a non-SQLite file");
        };
        let resolved = open_workspace_with_cli(&overrides)
            .await
            .expect("resolve-only succeeds");
        let Err(e2) = open_resolved_with_storage(resolved).await else {
            panic!("the resolve-then-open path must fail on a non-SQLite file");
        };
        assert!(matches!(e1, ConfigError::DbOpenFailed { .. }), "{e1:?}");
        assert!(matches!(e2, ConfigError::DbOpenFailed { .. }), "{e2:?}");
        assert_eq!(e1.to_string(), e2.to_string());
        assert_eq!(e1.code(), e2.code());
        assert_eq!(
            std::fs::read(&db).expect("read"),
            payload,
            "bytes unchanged"
        );
    }
}

/// Assert the JSONL path of `ws` resolves to `expected` (so a wrong config key fails loudly).
async fn assert_jsonl_is(ws: &Workspace, expected: &Path) {
    let resolved = open_workspace_with_cli(&ws.overrides())
        .await
        .expect("resolve");
    assert_eq!(
        resolved.paths.jsonl_path, expected,
        "precondition: the workspace's JSONL path"
    );
}

/// (kpe4) The JSONL path IS the database (config key, hard link, `--db …/issues.jsonl`): doctor
/// never opens and closes the db inode inside the session.
#[tokio::test]
async fn kpe4_jsonl_that_is_the_db_keeps_the_shared_lock() {
    // (i) `config.toml` sets `jsonl_export_filename = "unblock.db"`.
    let ws = Workspace::migrated_with(Some("jsonl_export_filename = \"unblock.db\"\n"), None).await;
    assert_jsonl_is(&ws, &ws.db).await;
    let mut holder = ready_holder("kpe4i", &ws);
    let doctored = doctor_and_observe("kpe4i", &ws, &mut holder, SAME_FILE_MUTANT);
    assert_eq!(doctored, HEALTHY_LABELS);
    holder.quit();

    // (ii) `issues.jsonl` is a hard link to `unblock.db`.
    let ws = Workspace::migrated().await;
    std::fs::hard_link(&ws.db, ws.unblock.join("issues.jsonl")).expect("hard link");
    let mut holder = ready_holder("kpe4ii", &ws);
    let doctored = doctor_and_observe("kpe4ii", &ws, &mut holder, SAME_FILE_MUTANT);
    assert_eq!(doctored, HEALTHY_LABELS);
    holder.quit();

    // (iii) `--db <dir>/issues.jsonl`: the db itself carries the JSONL name.
    let ws = Workspace::migrated_with(None, Some("issues.jsonl")).await;
    assert_jsonl_is(&ws, &ws.db).await;
    let mut holder = ready_holder("kpe4iii", &ws);
    let doctored = doctor_and_observe("kpe4iii", &ws, &mut holder, SAME_FILE_MUTANT);
    assert_eq!(doctored, HEALTHY_LABELS);
    holder.quit();
}

/// (kpe6) The JSONL path is a sidecar (`-shm`/`-wal` by config, a hard link to `-shm`): the db SHARED
/// lock AND the `-shm` DMS lock are kept, and no conflict-marker finding appears. NOTE: the `-wal`
/// shape (`kpe6ii`) is NOT lock evidence for the `-wal` itself — `SQLite` holds no `fcntl` lock on
/// `-wal`, so a close there drops nothing observable; that shape is guarded by the health unit cell
/// `conflict_scan_skips_the_db_itself`.
#[tokio::test]
async fn kpe6_jsonl_that_is_a_sidecar_keeps_the_sidecar_locks() {
    const WHY: &str = "the JSONL conflict scan opened a sidecar inside the session (D29 clause F6)";
    for (cell, config) in [
        ("kpe6i", "jsonl_export_filename = \"unblock.db-shm\"\n"),
        ("kpe6ii", "jsonl_export_filename = \"unblock.db-wal\"\n"),
    ] {
        let ws = Workspace::migrated_with(Some(config), None).await;
        let suffix = if cell == "kpe6i" { "-shm" } else { "-wal" };
        assert_jsonl_is(&ws, &ws.sidecar(suffix)).await;
        let mut holder = ready_holder(cell, &ws);
        assert_shm_dms_held(cell, "baseline", &ws.sidecar("-shm"), &holder, WHY);
        let doctored = doctor_and_observe(cell, &ws, &mut holder, WHY);
        assert!(!doctored.contains("jsonl_conflict_markers"), "{doctored}");
        assert_shm_dms_held(cell, "after-doctor", &ws.sidecar("-shm"), &holder, WHY);
        holder.quit();
    }

    // (iii) `issues.jsonl` hard-linked to `-shm` while the holder is connected (after READY: the
    // `-shm` exists only while a connection is open).
    let ws = Workspace::migrated().await;
    let mut holder = ready_holder("kpe6iii", &ws);
    std::fs::hard_link(ws.sidecar("-shm"), ws.unblock.join("issues.jsonl")).expect("hard link");
    assert_shm_dms_held("kpe6iii", "baseline", &ws.sidecar("-shm"), &holder, WHY);
    let doctored = doctor_and_observe("kpe6iii", &ws, &mut holder, WHY);
    assert!(!doctored.contains("jsonl_conflict_markers"), "{doctored}");
    assert_shm_dms_held("kpe6iii", "after-doctor", &ws.sidecar("-shm"), &holder, WHY);
    holder.quit();
}

/// (kpe6m) POSITIVE CONTROL of the `-shm` witness: closing a descriptor on `-shm` drops the DMS lock.
#[tokio::test]
async fn kpe6m_shm_close_drops_the_dms_lock() {
    let ws = Workspace::migrated().await;
    let mut holder = ready_holder("kpe6m", &ws);
    let shm = ws.sidecar("-shm");
    assert_shm_dms_held("kpe6m", "baseline", &shm, &holder, "precondition");
    assert_eq!(holder.send("PROBE-SHM-CLOSE"), "SHM-CLOSED");
    let kind = posix_locks::shm_dms(&shm, holder.pid());
    eprintln!(
        "UB2VP-MEASURE leg={} cell=kpe6m point=after-shm-close holder_pid={} shm_dms={kind:?}",
        posix_locks::leg(),
        holder.pid()
    );
    assert_eq!(
        kind, None,
        "closing a -shm descriptor must drop the DMS lock (the witness sees a loss)"
    );
}

/// (kpe7) The race branch of `SessionFiles::open_verified` (what a rename between the stat pre-check
/// and the open produces) keeps the descriptor OPEN: a "close instead of leak" mutant goes red.
#[cfg(feature = "sync")]
#[tokio::test]
async fn kpe7_verified_open_never_closes_a_session_file() {
    let ws = Workspace::migrated().await;
    let mut holder = ready_holder("kpe7", &ws);
    let db_link = ws.unblock.join("db-link.jsonl");
    std::fs::hard_link(&ws.db, &db_link).expect("hard link to the db");
    assert_eq!(
        holder.send(&format!("OPEN-VERIFIED {}", db_link.display())),
        "VERIFIED none"
    );
    let why = "SessionFiles::open_verified closed a session-file descriptor (D29 clause F6)";
    assert_shared_and_busy(ENTRY, "kpe7", "after-db-verify", &ws.db, &holder, why);

    let shm = ws.sidecar("-shm");
    let shm_link = ws.unblock.join("shm-link.jsonl");
    std::fs::hard_link(&shm, &shm_link).expect("hard link to -shm");
    assert_eq!(
        holder.send(&format!("OPEN-VERIFIED {}", shm_link.display())),
        "VERIFIED none"
    );
    assert_shm_dms_held("kpe7", "after-shm-verify", &shm, &holder, why);
    holder.quit();
}

/// (`kpe_p`) The ported `/proc/locks` parser reads canned lines: it keeps the POSIX lines of one inode
/// and pid, decodes the hex device, skips a blocked waiter's `->` line, reads `EOF` as length 0,
/// dedups a torn read's repeated line and falls back to the inode alone when no line carries the
/// `stat` device (btrfs). Verbatim port of main's `fd_proc_locks_parser_reads_canned_lines`.
#[cfg(target_os = "linux")]
#[test]
fn kpe_p_proc_locks_parser_reads_canned_lines() {
    use posix_locks::{HeldLock, LockKind, dev_major_minor, parse_proc_locks, raw_lines_on_inode};

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
