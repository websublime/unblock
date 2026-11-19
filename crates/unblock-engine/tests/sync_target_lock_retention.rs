//! PRD §4 D5 amendment (`ub-w1j`, v1.0.3): a sync target that is the session database or one of its
//! sidecars is refused, and the refusal keeps the session's `SQLite` locks and the database itself.
//!
//! The holder (`doctor_lock_holder`) keeps a real `Session` open and runs the engine's
//! `import_jsonl` / `import_bd` / `export_jsonl` on command. Before the fix an import of a hard link to
//! the database opened and closed a descriptor on it inside the session (dropping SHARED, the `ub-2vp`
//! mechanism), and an export to a database named `issues.jsonl` renamed a JSONL over it. Every cell
//! first asserts the baseline (SHARED held + a fresh observer BUSY).
#![cfg(all(unix, feature = "sync"))]

mod common;

use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

use common::lock_harness::{Holder, Workspace, assert_shared_and_busy, assert_shm_dms_held};

/// This binary's observer entry (see `common::lock_observer`).
const ENTRY: &str = "kpe_observer_entry";

const MUTANT: &str =
    "a sync action opened and closed a session file, or replaced the database (D5 amendment)";

/// The observer process body (see `common::lock_observer`).
#[test]
#[ignore = "observer process entry point; spawned by the w1j cells"]
fn kpe_observer_entry() {
    common::lock_observer::observer_main();
}

fn ready_holder(cell: &str, ws: &Workspace) -> Holder {
    let holder = Holder::spawn(ws);
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

/// (w1j0) `IMPORT` / `IMPORT-BD` of a hard link to `unblock.db` are refused and keep SHARED.
#[tokio::test]
async fn w1j0_refused_import_keeps_the_shared_lock() {
    let ws = Workspace::migrated().await;
    let alias = ws.unblock.join("alias.jsonl");
    std::fs::hard_link(&ws.db, &alias).expect("hard link");
    let mut holder = ready_holder("w1j0", &ws);
    assert_refused(
        &holder.send(&format!("IMPORT {}", alias.display())),
        "IMPORT",
    );
    assert_shared_and_busy(ENTRY, "w1j0", "after-import", &ws.db, &holder, MUTANT);
    assert_refused(
        &holder.send(&format!("IMPORT-BD {}", alias.display())),
        "IMPORT-BD",
    );
    assert_shared_and_busy(ENTRY, "w1j0", "after-import-bd", &ws.db, &holder, MUTANT);
    holder.quit();
}

/// (w1j1) With the database named `issues.jsonl` (`--db`), `EXPORT` to it is refused: the database's
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

/// (w1j2) `IMPORT` of a hard link to `unblock.db-shm` (created after READY) is refused and keeps the
/// `-shm` DMS lock.
#[tokio::test]
async fn w1j2_refused_sidecar_import_keeps_the_shm_locks() {
    let ws = Workspace::migrated().await;
    let mut holder = ready_holder("w1j2", &ws);
    let shm = ws.sidecar("-shm");
    let alias = ws.unblock.join("alias.jsonl");
    std::fs::hard_link(&shm, &alias).expect("hard link to -shm");
    assert_shm_dms_held("w1j2", "baseline", &shm, &holder, "precondition");
    assert_refused(
        &holder.send(&format!("IMPORT {}", alias.display())),
        "IMPORT of -shm",
    );
    assert_shm_dms_held("w1j2", "after-import", &shm, &holder, MUTANT);
    assert_shared_and_busy(ENTRY, "w1j2", "after-import", &ws.db, &holder, MUTANT);
    holder.quit();
}

/// (w1j5) A SYMLINKED database (`.unblock/unblock.db` → `.unblock/store/unblock.db`): `SQLite` keeps
/// the live `-shm` next to the TARGET. A JSONL-named hard link to that `-shm` is refused by `IMPORT`,
/// skipped by `DOCTOR`'s conflict scan, and the `-shm` DMS lock and SHARED are kept (verify1 finding:
/// the session set is also derived from the canonical db path).
#[tokio::test]
async fn w1j5_symlinked_db_target_sidecars_keep_their_locks() {
    let ws = Workspace::migrated().await;
    let store = ws.unblock.join("store");
    std::fs::create_dir(&store).expect("mkdir store");
    let real = store.join("unblock.db");
    std::fs::rename(&ws.db, &real).expect("move the db");
    std::os::unix::fs::symlink(&real, &ws.db).expect("symlink the db");
    let mut holder = ready_holder("w1j5", &ws);
    let shm = store.join("unblock.db-shm");
    assert!(
        shm.exists(),
        "precondition: SQLite keeps the -shm next to the symlink's target"
    );
    assert_shm_dms_held("w1j5", "baseline", &shm, &holder, "precondition");
    let alias = ws.unblock.join("alias.jsonl");
    std::fs::hard_link(&shm, &alias).expect("hard link to the target's -shm");
    assert_refused(
        &holder.send(&format!("IMPORT {}", alias.display())),
        "IMPORT of the target's -shm",
    );
    assert_shm_dms_held("w1j5", "after-import", &shm, &holder, MUTANT);
    std::fs::hard_link(&shm, ws.unblock.join("issues.jsonl")).expect("issues.jsonl -> -shm");
    let doctored = holder.send("DOCTOR");
    assert!(
        doctored.starts_with("DOCTORED ") && !doctored.contains("jsonl_conflict_markers"),
        "{doctored}"
    );
    assert_shm_dms_held("w1j5", "after-doctor", &shm, &holder, MUTANT);
    assert_shared_and_busy(ENTRY, "w1j5", "after-doctor", &ws.db, &holder, MUTANT);
    holder.quit();
}
