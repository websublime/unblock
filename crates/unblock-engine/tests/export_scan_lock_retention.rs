//! PRD §4 D58 amendment (`ub-c15`, the D5 amendment applied to `doctor`): `doctor`'s export scan and
//! `doctor --repair`'s online `recover()` skip an export that is the session database or one of its
//! sidecars, report it (`export_is_session_file`), and keep the session's `SQLite` locks.
//!
//! The holder (`session_lock_holder`) opens a `Session` in the cli's doctor order (`--mode doctor`:
//! resolve only → `gather_preopen` → read-only open → one read) or its recover order (`--mode
//! recover`: non-migrating open → one read), then runs the real `Session::doctor` / `recover` on
//! command. The parent asserts from outside that the holder still holds SHARED (`F_GETLK`, plus
//! `/proc/locks` on Linux), that the `-shm` DMS lock is held, and that a fresh observer is BUSY.
//! Without the fix the scan of such an export opens and closes a descriptor on the session file.
#![cfg(all(unix, feature = "health"))]

mod common;

use std::path::Path;

use common::lock_harness::{Holder, Workspace, assert_shared_and_busy, assert_shm_dms_held};
use unblock_config::open_workspace_with_cli;

/// This binary's observer entry (see `common::lock_observer`).
const ENTRY: &str = "kpe_observer_entry";

const MUTANT: &str =
    "the export scan opened and closed a session file inside the session (D58 amendment)";

/// The observer process body (see `common::lock_observer`).
#[test]
#[ignore = "observer process entry point; spawned by the lock cells"]
fn kpe_observer_entry() {
    common::lock_observer::observer_main();
}

/// Assert SHARED, the `-shm` DMS lock and a BUSY observer at `point`.
fn assert_locks(cell: &str, point: &str, ws: &Workspace, shm: &Path, holder: &Holder, why: &str) {
    assert_shm_dms_held(cell, point, shm, holder, why);
    assert_shared_and_busy(ENTRY, cell, point, &ws.db, holder, why);
}

/// Spawn the holder in `mode` and assert the baseline locks.
fn ready_holder(cell: &str, ws: &Workspace, mode: &str, shm: &Path) -> Holder {
    let holder = Holder::spawn(ws, mode);
    assert_locks(cell, "baseline", ws, shm, &holder, "precondition");
    holder
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

/// `DOCTOR` then `READ`, with the locks asserted after each; returns the `DOCTORED` reply, which
/// must report the skip and no scan finding.
fn doctor_and_observe(cell: &str, ws: &Workspace, shm: &Path, holder: &mut Holder) -> String {
    let doctored = holder.send("DOCTOR");
    assert!(
        doctored.starts_with("DOCTORED "),
        "{cell}: {doctored:?} (holder stderr {})",
        holder.stderr()
    );
    assert!(
        doctored.contains("export_is_session_file")
            && !doctored.contains("jsonl_conflict_markers")
            && !doctored.contains("jsonl_unscannable"),
        "{cell}: the session-file export must be skipped and reported, got {doctored:?}"
    );
    assert_locks(cell, "after-doctor", ws, shm, holder, MUTANT);
    let read = holder.send("READ");
    assert!(read.starts_with("READ "), "{cell}: {read:?}");
    assert_locks(cell, "after-read", ws, shm, holder, MUTANT);
    doctored
}

/// `RECOVER`, with the locks asserted after it: the reply is `nothing to repair online` and no
/// regeneration, and no run directory exists (recover carries no advisories).
fn recover_and_observe(cell: &str, ws: &Workspace, shm: &Path, holder: &mut Holder) {
    let recovered = holder.send("RECOVER");
    assert_eq!(
        recovered,
        "RECOVERED recover=nothing to repair online",
        "{cell}: (holder stderr {})",
        holder.stderr()
    );
    assert!(
        !ws.unblock.join(".recovery").exists(),
        "{cell}: recover created a run directory"
    );
    assert_locks(cell, "after-recover", ws, shm, holder, MUTANT);
}

/// (D0) The export IS the database — `jsonl_export_filename = "unblock.db"`, a hard link
/// `issues.jsonl` → `unblock.db`, `--db <dir>/issues.jsonl`, and a symlink `issues.jsonl` →
/// `unblock.db`: `doctor` skips and reports it and keeps SHARED, the DMS lock and a BUSY observer.
#[tokio::test]
async fn c15d0_doctor_skips_an_export_that_is_the_database_and_keeps_the_shared_lock() {
    // (i) `config.toml` names the database as the export.
    let ws = Workspace::migrated_with(Some("jsonl_export_filename = \"unblock.db\"\n"), None).await;
    assert_jsonl_is(&ws, &ws.db).await;
    let shm = ws.sidecar(unblock_sync::SHM_SUFFIX);
    let mut holder = ready_holder("c15d0i", &ws, "doctor", &shm);
    doctor_and_observe("c15d0i", &ws, &shm, &mut holder);
    assert_eq!(holder.send("DOCTOR"), "DOCTOR-SPENT");
    holder.quit();

    // (ii) `issues.jsonl` is a hard link to `unblock.db`.
    let ws = Workspace::migrated().await;
    std::fs::hard_link(&ws.db, ws.unblock.join("issues.jsonl")).expect("hard link");
    let shm = ws.sidecar(unblock_sync::SHM_SUFFIX);
    let mut holder = ready_holder("c15d0ii", &ws, "doctor", &shm);
    doctor_and_observe("c15d0ii", &ws, &shm, &mut holder);
    holder.quit();

    // (iii) `--db <dir>/issues.jsonl`: the database itself carries the export's name.
    let ws = Workspace::migrated_with(None, Some("issues.jsonl")).await;
    assert_jsonl_is(&ws, &ws.db).await;
    let shm = ws.sidecar(unblock_sync::SHM_SUFFIX);
    let mut holder = ready_holder("c15d0iii", &ws, "doctor", &shm);
    doctor_and_observe("c15d0iii", &ws, &shm, &mut holder);
    holder.quit();

    // (iv) `issues.jsonl` is a symlink to `unblock.db` (the scanner follows links).
    let ws = Workspace::migrated().await;
    std::os::unix::fs::symlink(&ws.db, ws.unblock.join("issues.jsonl")).expect("symlink");
    let shm = ws.sidecar(unblock_sync::SHM_SUFFIX);
    let mut holder = ready_holder("c15d0iv", &ws, "doctor", &shm);
    doctor_and_observe("c15d0iv", &ws, &shm, &mut holder);
    holder.quit();
}

/// (D1) The export is a sidecar — `jsonl_export_filename = "unblock.db-shm"` or `"unblock.db-wal"`,
/// or a hard link `issues.jsonl` → `-shm` made after READY: `doctor` skips and reports it and keeps
/// the `-shm` DMS lock and SHARED. The `-wal` shape is not lock evidence for the `-wal` itself
/// (`SQLite` holds no `fcntl` lock there); it pins the skip.
#[tokio::test]
async fn c15d1_doctor_skips_an_export_that_is_a_sidecar_and_keeps_the_shm_lock() {
    for (cell, suffix) in [
        ("c15d1i", unblock_sync::SHM_SUFFIX),
        ("c15d1ii", unblock_sync::WAL_SUFFIX),
    ] {
        let config = format!("jsonl_export_filename = \"unblock.db{suffix}\"\n");
        let ws = Workspace::migrated_with(Some(&config), None).await;
        assert_jsonl_is(&ws, &ws.sidecar(suffix)).await;
        let shm = ws.sidecar(unblock_sync::SHM_SUFFIX);
        let mut holder = ready_holder(cell, &ws, "doctor", &shm);
        doctor_and_observe(cell, &ws, &shm, &mut holder);
        holder.quit();
    }

    // (iii) A hard link made while the holder is connected (the `-shm` exists only then). The
    // doctor-mode holder gathered its pre-open facts before the link existed; `DOCTOR`'s own export
    // scan is the one that meets it.
    let ws = Workspace::migrated().await;
    let shm = ws.sidecar(unblock_sync::SHM_SUFFIX);
    let mut holder = ready_holder("c15d1iii", &ws, "doctor", &shm);
    std::fs::hard_link(&shm, ws.unblock.join("issues.jsonl")).expect("hard link to -shm");
    doctor_and_observe("c15d1iii", &ws, &shm, &mut holder);
    holder.quit();
}

/// (D2) A SYMLINKED database (`.unblock/unblock.db` → `.unblock/store/unblock.db`): `SQLite` keeps the
/// live `-shm` next to the TARGET, and `issues.jsonl` hard-linked to it is skipped and reported; the
/// target's `-shm` DMS lock and SHARED are kept.
#[tokio::test]
async fn c15d2_doctor_on_a_symlinked_database_skips_the_targets_shm() {
    let ws = Workspace::migrated().await;
    let store = ws.unblock.join("store");
    std::fs::create_dir(&store).expect("mkdir store");
    let real = store.join("unblock.db");
    std::fs::rename(&ws.db, &real).expect("move the db");
    std::os::unix::fs::symlink(&real, &ws.db).expect("symlink the db");
    let shm = unblock_sync::sidecar(&real, unblock_sync::SHM_SUFFIX);
    let mut holder = ready_holder("c15d2", &ws, "doctor", &shm);
    std::fs::hard_link(&shm, ws.unblock.join("issues.jsonl")).expect("issues.jsonl -> -shm");
    doctor_and_observe("c15d2", &ws, &shm, &mut holder);
    holder.quit();
}

/// (D3) `recover()` with the export resolving to the database (a hard link, then
/// `--db <dir>/issues.jsonl`): `nothing to repair online`, no regeneration, no run directory, the
/// database's identity unchanged, and SHARED, the DMS lock and a BUSY observer kept.
#[tokio::test]
async fn c15d3_recover_skips_an_export_that_is_the_database_and_keeps_the_shared_lock() {
    use std::os::unix::fs::MetadataExt as _;

    let hard_link = Workspace::migrated().await;
    std::fs::hard_link(&hard_link.db, hard_link.unblock.join("issues.jsonl")).expect("hard link");
    let same_name = Workspace::migrated_with(None, Some("issues.jsonl")).await;
    for (cell, ws) in [("c15d3ii", hard_link), ("c15d3iii", same_name)] {
        let ino = std::fs::metadata(&ws.db).expect("stat").ino();
        let shm = ws.sidecar(unblock_sync::SHM_SUFFIX);
        let mut holder = ready_holder(cell, &ws, "recover", &shm);
        recover_and_observe(cell, &ws, &shm, &mut holder);
        assert_eq!(
            std::fs::metadata(&ws.db).expect("stat").ino(),
            ino,
            "{cell}: the database was replaced"
        );
        holder.quit();
    }
}

/// (D4) `recover()` with `issues.jsonl` hard-linked to `-shm` after READY: `nothing to repair
/// online`, no run directory, and the `-shm` DMS lock and SHARED kept.
#[tokio::test]
async fn c15d4_recover_skips_an_export_that_is_a_sidecar_and_keeps_the_shm_lock() {
    let ws = Workspace::migrated().await;
    let shm = ws.sidecar(unblock_sync::SHM_SUFFIX);
    let mut holder = ready_holder("c15d4", &ws, "recover", &shm);
    std::fs::hard_link(&shm, ws.unblock.join("issues.jsonl")).expect("hard link to -shm");
    recover_and_observe("c15d4", &ws, &shm, &mut holder);
    holder.quit();
}
