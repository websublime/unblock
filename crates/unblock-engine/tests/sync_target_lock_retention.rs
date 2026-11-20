//! PRD §4 D5 amendment (`ub-c15`, v1.1): a sync target that is the session database or one of its
//! sidecars is refused, and the refusal keeps the session's `SQLite` locks and the database itself.
//!
//! The holder (`session_lock_holder --mode sync`, the MCP server's open order) keeps a real
//! `Session` open and runs the engine's `import_jsonl` (with and without `dry_run`) / `import_bd` /
//! `export_jsonl` on command. Without the fix an import of a hard link to the database opens and
//! closes a descriptor on it inside the session (dropping SHARED, the `ub-2vp` mechanism), and an
//! export to a database named `issues.jsonl` renames a JSONL over it. Every cell first asserts the
//! baseline (SHARED held + a fresh observer BUSY); `c15c1m` and `kpe6m` are the in-tree positive
//! controls that the instrument SEES a lock loss.
#![cfg(all(unix, feature = "sync"))]

mod common;

use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

use common::lock_harness::{
    Holder, Workspace, assert_shared_and_busy, assert_shm_dms_held, measure, observe,
};
use common::posix_locks;

/// This binary's observer entry (see `common::lock_observer`).
const ENTRY: &str = "kpe_observer_entry";

const MUTANT: &str =
    "a sync action opened and closed a session file, or replaced the database (D5 amendment)";

/// The three import commands: each refuses a session-file source identically.
const IMPORTS: [&str; 3] = ["IMPORT", "IMPORT-DRY", "IMPORT-BD"];

/// The observer process body (see `common::lock_observer`).
#[test]
#[ignore = "observer process entry point; spawned by the lock cells"]
fn kpe_observer_entry() {
    common::lock_observer::observer_main();
}

fn ready_holder(cell: &str, ws: &Workspace) -> Holder {
    let holder = Holder::spawn(ws, "sync");
    assert_shared_and_busy(ENTRY, cell, "baseline", &ws.db, &holder, "precondition");
    holder
}

fn assert_refused(reply: &str, what: &str) {
    assert!(
        reply.starts_with("SYNC-REFUSED PATH_TRAVERSAL") && reply.contains("SessionDatabase"),
        "{what} must be refused as PATH_TRAVERSAL/SessionDatabase, got {reply:?}"
    );
}

fn identity_and_bytes(p: &Path) -> ((u64, u64), Vec<u8>) {
    let meta = std::fs::metadata(p).expect("stat");
    ((meta.dev(), meta.ino()), std::fs::read(p).expect("read"))
}

/// (L0) `IMPORT`, `IMPORT-DRY` and `IMPORT-BD` of a hard link to `unblock.db` are refused and keep
/// SHARED.
#[tokio::test]
async fn w1j0_refused_import_keeps_the_shared_lock() {
    let ws = Workspace::migrated().await;
    let alias = ws.unblock.join("alias.jsonl");
    std::fs::hard_link(&ws.db, &alias).expect("hard link");
    let mut holder = ready_holder("w1j0", &ws);
    for command in IMPORTS {
        assert_refused(
            &holder.send(&format!("{command} {}", alias.display())),
            command,
        );
        let point = format!("after-{}", command.to_lowercase());
        assert_shared_and_busy(ENTRY, "w1j0", &point, &ws.db, &holder, MUTANT);
    }
    holder.quit();
}

/// (L1) With the database named `issues.jsonl` (`--db`), `EXPORT` to it is refused: the database's
/// identity and bytes are unchanged, reads still work, and the observer is BUSY.
#[tokio::test]
async fn w1j1_refused_export_keeps_the_database() {
    let ws = Workspace::migrated_with(None, Some("issues.jsonl")).await;
    let mut holder = ready_holder("w1j1", &ws);
    let before = identity_and_bytes(&ws.db);
    assert_refused(
        &holder.send(&format!("EXPORT {}", ws.db.display())),
        "EXPORT",
    );
    assert_eq!(
        identity_and_bytes(&ws.db),
        before,
        "the database was replaced — {MUTANT}"
    );
    assert!(holder.send("READ").starts_with("READ "));
    assert_shared_and_busy(ENTRY, "w1j1", "after-export", &ws.db, &holder, MUTANT);
    holder.quit();
}

/// (L2) `IMPORT`, `IMPORT-DRY` and `IMPORT-BD` of a hard link to `unblock.db-shm` (created after
/// READY: the `-shm` exists only while a connection is open) are refused and keep the `-shm` DMS
/// lock and SHARED.
#[tokio::test]
async fn w1j2_refused_sidecar_import_keeps_the_shm_locks() {
    let ws = Workspace::migrated().await;
    let mut holder = ready_holder("w1j2", &ws);
    let shm = ws.sidecar(unblock_sync::SHM_SUFFIX);
    let alias = ws.unblock.join("alias.jsonl");
    std::fs::hard_link(&shm, &alias).expect("hard link to -shm");
    assert_shm_dms_held("w1j2", "baseline", &shm, &holder, "precondition");
    for command in IMPORTS {
        assert_refused(
            &holder.send(&format!("{command} {}", alias.display())),
            &format!("{command} of -shm"),
        );
        let point = format!("after-{}", command.to_lowercase());
        assert_shm_dms_held("w1j2", &point, &shm, &holder, MUTANT);
        assert_shared_and_busy(ENTRY, "w1j2", &point, &ws.db, &holder, MUTANT);
    }
    holder.quit();
}

/// (L3) A SYMLINKED database (`.unblock/unblock.db` → `.unblock/store/unblock.db`): `SQLite` keeps the
/// live `-shm` next to the TARGET. A JSONL-named hard link to that `-shm` is refused by every import
/// command and by `EXPORT`, and the `-shm` DMS lock and SHARED are kept (`of_database` also stats
/// the canonical db path).
#[tokio::test]
async fn w1j5_symlinked_db_target_sidecars_keep_their_locks() {
    let ws = Workspace::migrated().await;
    let store = ws.unblock.join("store");
    std::fs::create_dir(&store).expect("mkdir store");
    let real = store.join("unblock.db");
    std::fs::rename(&ws.db, &real).expect("move the db");
    std::os::unix::fs::symlink(&real, &ws.db).expect("symlink the db");
    let mut holder = ready_holder("w1j5", &ws);
    let shm = unblock_sync::sidecar(&real, unblock_sync::SHM_SUFFIX);
    assert!(
        shm.exists(),
        "precondition: SQLite keeps the -shm next to the symlink's target"
    );
    assert_shm_dms_held("w1j5", "baseline", &shm, &holder, "precondition");
    let alias = ws.unblock.join("alias.jsonl");
    std::fs::hard_link(&shm, &alias).expect("hard link to the target's -shm");
    for command in IMPORTS.iter().copied().chain(["EXPORT"]) {
        let before = identity_and_bytes(&shm);
        assert_refused(
            &holder.send(&format!("{command} {}", alias.display())),
            &format!("{command} of the target's -shm"),
        );
        assert_eq!(
            identity_and_bytes(&shm).0,
            before.0,
            "{command} replaced the -shm — {MUTANT}"
        );
        let point = format!("after-{}", command.to_lowercase());
        assert_shm_dms_held("w1j5", &point, &shm, &holder, MUTANT);
        assert_shared_and_busy(ENTRY, "w1j5", &point, &ws.db, &holder, MUTANT);
    }
    holder.quit();
}

/// (L4) The race branch of `SessionFiles::open_verified` (what a rename between the stat pre-check
/// and the open produces) keeps the descriptor OPEN: a "close instead of keep" mutant goes red. The
/// only LOCK-retention proof of the primitive.
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
    let why = "SessionFiles::open_verified closed a session-file descriptor (D5 amendment)";
    assert_shared_and_busy(ENTRY, "kpe7", "after-db-verify", &ws.db, &holder, why);

    let shm = ws.sidecar(unblock_sync::SHM_SUFFIX);
    let shm_link = ws.unblock.join("shm-link.jsonl");
    std::fs::hard_link(&shm, &shm_link).expect("hard link to -shm");
    assert_eq!(
        holder.send(&format!("OPEN-VERIFIED {}", shm_link.display())),
        "VERIFIED none"
    );
    assert_shm_dms_held("kpe7", "after-shm-verify", &shm, &holder, why);
    holder.quit();
}

/// (L5) POSITIVE CONTROL of the `-shm` witness: closing a descriptor on `-shm` drops the DMS lock.
#[tokio::test]
async fn kpe6m_shm_close_drops_the_dms_lock() {
    let ws = Workspace::migrated().await;
    let mut holder = ready_holder("kpe6m", &ws);
    let shm = ws.sidecar(unblock_sync::SHM_SUFFIX);
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

/// (L6) CONTROL: the holder keeps SHARED and the observer is BUSY across a no-op.
#[tokio::test]
async fn c15c0_control_noop_keeps_the_shared_lock() {
    let ws = Workspace::migrated().await;
    let mut holder = ready_holder("c15c0", &ws);
    assert_eq!(holder.send("NOOP"), "OK");
    assert_shared_and_busy(ENTRY, "c15c0", "after-noop", &ws.db, &holder, "control");
    holder.quit();
}

/// (L7) POSITIVE CONTROL of the SHARED witness in this harness: the holder opens and closes a
/// descriptor on the database (the hazard itself), and the instrument must see SHARED lost and the
/// observer `LOCKED`.
#[tokio::test]
async fn c15c1m_db_close_drops_the_shared_lock() {
    let ws = Workspace::migrated().await;
    let mut holder = ready_holder("c15c1m", &ws);
    assert_eq!(holder.send("PROBE-DB-CLOSE"), "DB-CLOSED");
    let pid = holder.pid();
    let observation = posix_locks::shared_range(&ws.db, pid);
    let observed = observe(ENTRY, &ws.db);
    let observation = measure(
        "c15c1m",
        "after-db-close",
        pid,
        observation,
        Some(&observed),
    );
    assert_eq!(
        observation.kind(),
        None,
        "closing a database descriptor must drop SHARED (the instrument sees a loss)"
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
