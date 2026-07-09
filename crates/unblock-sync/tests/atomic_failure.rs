//! Atomic-write failure-injection integration (NFR-4/5, DRIFT-1).
//!
//! `write_atomic` is crate-private, so these tests drive it through the public `export_jsonl` path
//! and assert the observable durability guarantees:
//! - the 3 PREFLIGHT tests (default build): a rejected export leaves any existing target
//!   byte-identical and removes every orphan temp; a successful export leaves NO `.tmp` sibling.
//! - the 5 per-stage SEAM tests (`--features fault-injection`, T3.4/D30): arming each of the five
//!   fault points and asserting the SPLIT-by-rename-ordering guarantee — Write/Flush/SyncAll/Rename
//!   (PRE-rename) leave the original byte-identical + zero orphan `*.tmp`; `ParentDirFsync` (POST-rename)
//!   applies the NEW content, reports `Err`, and leaves no orphan temp.
//! - the out-of-process SIGKILL test (unix): the atomic temp->fsync->rename keeps the target a COMPLETE
//!   valid JSONL across a hard kill (never a torn partial write).
//!
//! The seam tests arm a PROCESS-GLOBAL fault plan, so — ONLY under `--features fault-injection` —
//! every in-process `write_atomic` test serializes on `SEAM_LOCK` so no concurrent export observes an
//! armed fault. (Import failure-injection is NOT an fs seam: import applies via one
//! `Storage::create_issues` transaction, so it rides the DB-transaction rollback -> zero-rows proof —
//! exercised by `unblock-engine/tests/create_bulk.rs` + `import::tests` — never this seam.)

use std::path::Path;

use unblock_sync::{ExportOptions, export_jsonl};

#[cfg(feature = "fault-injection")]
use unblock_sync::{FaultPoint, SyncError, arm_fault, clear_faults};

mod fake;
use fake::{FakeStorage, sample_issue};

/// Serializes every in-process `write_atomic` test when the process-global fault plan can be armed
/// (`--features fault-injection`). A `tokio::sync::Mutex` (held across the export `await`, so NOT a
/// `std::sync::Mutex` — that would trip `clippy::await_holding_lock`).
#[cfg(feature = "fault-injection")]
static SEAM_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Count the `*.tmp` files in a directory (orphan-temp detector).
fn tmp_count(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .expect("read_dir")
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .count()
}

#[tokio::test]
async fn rejected_export_leaves_original_intact_and_no_orphan_temp() {
    #[cfg(feature = "fault-injection")]
    let _serial = SEAM_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let unblock = dir.path().join(".unblock");
    std::fs::create_dir_all(&unblock).unwrap();
    let target = unblock.join("issues.jsonl");
    std::fs::write(&target, "ORIGINAL\n").unwrap();

    // An external (out-of-confine) target is rejected at preflight — BEFORE any temp is created.
    let external = dir.path().join("outside.jsonl");
    let storage = FakeStorage::with_issues(vec![sample_issue("ub-1")]);
    let err = export_jsonl(&storage, &external, &unblock, &ExportOptions::default())
        .await
        .expect_err("external rejected");
    assert!(
        !external.exists(),
        "no file written to the rejected path: {err:?}"
    );

    // The pre-existing confined target is byte-identical, and no orphan temp was left behind.
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "ORIGINAL\n");
    assert_eq!(tmp_count(&unblock), 0, "no orphan temp file");
}

#[tokio::test]
async fn successful_export_leaves_no_temp_sibling() {
    #[cfg(feature = "fault-injection")]
    let _serial = SEAM_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let unblock = dir.path().join(".unblock");
    std::fs::create_dir_all(&unblock).unwrap();
    let target = unblock.join("issues.jsonl");

    let storage = FakeStorage::with_issues(vec![sample_issue("ub-1"), sample_issue("ub-2")]);
    let report = export_jsonl(&storage, &target, &unblock, &ExportOptions::default())
        .await
        .expect("export");
    assert_eq!(report.written, 2);
    assert!(target.exists());
    // The temp was renamed over the target — no `.tmp` sibling remains.
    assert_eq!(
        tmp_count(&unblock),
        0,
        "temp persisted as target, no orphan"
    );
    let content = std::fs::read_to_string(&target).unwrap();
    assert_eq!(content.lines().count(), 2);
}

#[tokio::test]
async fn overwrite_preserves_content_and_no_partial() {
    #[cfg(feature = "fault-injection")]
    let _serial = SEAM_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let unblock = dir.path().join(".unblock");
    std::fs::create_dir_all(&unblock).unwrap();
    let target = unblock.join("issues.jsonl");
    std::fs::write(&target, "STALE LINE\n").unwrap();

    let storage = FakeStorage::with_issues(vec![sample_issue("ub-1")]);
    export_jsonl(&storage, &target, &unblock, &ExportOptions::default())
        .await
        .expect("export");
    let content = std::fs::read_to_string(&target).unwrap();
    // Atomic replace: the stale content is gone, exactly one fresh line, no partial concatenation.
    assert!(!content.contains("STALE LINE"), "content: {content}");
    assert_eq!(content.lines().count(), 1);
    assert_eq!(tmp_count(&unblock), 0);
}

// ---- T3.4/D30 per-stage fault-injection SEAM tests (`--features fault-injection`) ----

/// Drive a PRE-rename fault (Write/Flush/SyncAll/Rename): the export fails BEFORE the atomic rename,
/// so the pre-existing target is byte-identical and the RAII guard removes the orphan temp.
///
/// Non-vacuous: with the fault seam compiled out (or the guard removed) the export would SUCCEED, so
/// `expect_err` would fail.
#[cfg(feature = "fault-injection")]
async fn assert_pre_rename_fault(point: FaultPoint) {
    let dir = tempfile::tempdir().unwrap();
    let unblock = dir.path().join(".unblock");
    std::fs::create_dir_all(&unblock).unwrap();
    let target = unblock.join("issues.jsonl");
    std::fs::write(&target, "ORIGINAL\n").unwrap();

    let storage = FakeStorage::with_issues(vec![sample_issue("ub-1")]);
    clear_faults();
    arm_fault(point);
    let err = export_jsonl(&storage, &target, &unblock, &ExportOptions::default())
        .await
        .expect_err("an armed pre-rename fault must fail the export");
    clear_faults();

    assert!(
        matches!(err, SyncError::Io { .. }),
        "expected an Io error for {point:?}, got {err:?}"
    );
    // PRE-rename: the target never changed.
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "ORIGINAL\n",
        "the original is byte-identical after a {point:?} fault"
    );
    assert_eq!(
        tmp_count(&unblock),
        0,
        "the RAII guard removed the orphan temp after a {point:?} fault"
    );
}

#[cfg(feature = "fault-injection")]
#[tokio::test]
async fn fault_write_leaves_original_intact() {
    let _serial = SEAM_LOCK.lock().await;
    assert_pre_rename_fault(FaultPoint::Write).await;
}

#[cfg(feature = "fault-injection")]
#[tokio::test]
async fn fault_flush_leaves_original_intact() {
    let _serial = SEAM_LOCK.lock().await;
    assert_pre_rename_fault(FaultPoint::Flush).await;
}

#[cfg(feature = "fault-injection")]
#[tokio::test]
async fn fault_sync_all_leaves_original_intact() {
    let _serial = SEAM_LOCK.lock().await;
    assert_pre_rename_fault(FaultPoint::SyncAll).await;
}

#[cfg(feature = "fault-injection")]
#[tokio::test]
async fn fault_rename_leaves_original_intact() {
    let _serial = SEAM_LOCK.lock().await;
    assert_pre_rename_fault(FaultPoint::Rename).await;
}

/// `ParentDirFsync` fires POST-rename (`atomic.rs`): the new content is ALREADY applied (the temp
/// became the target), the op reports `Err` (durability unconfirmed), and there is no orphan temp.
#[cfg(feature = "fault-injection")]
#[tokio::test]
async fn fault_parent_dir_fsync_applies_new_content_but_reports_err() {
    let _serial = SEAM_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let unblock = dir.path().join(".unblock");
    std::fs::create_dir_all(&unblock).unwrap();
    let target = unblock.join("issues.jsonl");
    std::fs::write(&target, "ORIGINAL\n").unwrap();

    let storage = FakeStorage::with_issues(vec![sample_issue("ub-1")]);
    clear_faults();
    arm_fault(FaultPoint::ParentDirFsync);
    let err = export_jsonl(&storage, &target, &unblock, &ExportOptions::default())
        .await
        .expect_err("an armed ParentDirFsync fault reports Err (durability unconfirmed)");
    clear_faults();

    assert!(
        matches!(err, SyncError::Io { .. }),
        "expected an Io error, got {err:?}"
    );
    // POST-rename: the NEW content IS applied (the temp became the target); the original is gone.
    let content = std::fs::read_to_string(&target).unwrap();
    assert!(
        content.contains("ub-1"),
        "the post-rename target holds the new export: {content}"
    );
    assert!(
        !content.contains("ORIGINAL"),
        "the atomic rename replaced the original"
    );
    assert_eq!(
        tmp_count(&unblock),
        0,
        "the temp became the target — no orphan temp remains"
    );
}

// ---- T3.4/C5 out-of-process killed-write test (SIGKILL, unix) ----

/// The number of issues `export_kill` seeds — MUST match the helper's `SEED`.
#[cfg(unix)]
const KILL_SEED: usize = 800;

/// SIGKILL the `export_kill` helper mid-write: the atomic temp->fsync->rename keeps the target a
/// COMPLETE valid JSONL (never a torn partial write) — the real killed-export durability proof (NFR-4),
/// reusing the T3.2 raw `[[bin]]` precedent.
///
/// Non-vacuous: if `export_jsonl` wrote directly to the target instead of via temp->rename, a mid-write
/// SIGKILL would leave a truncated final line and the per-line JSON parse below would fail.
///
/// (The IMPORT-direction killed-write is the DB-transaction rollback -> zero-rows proof — the sibling
/// `unblock-storage/tests/shutdown_abandoned_tx.rs` SIGKILL-abandoned-tx test — not this fs path.)
#[cfg(unix)]
#[test]
fn sigkill_mid_export_leaves_a_complete_target_file() {
    use std::io::BufRead as _;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let dir = tempfile::tempdir().expect("tempdir");
    let unblock = dir.path().join(".unblock");
    std::fs::create_dir_all(&unblock).expect("create .unblock");
    let db_path = unblock.join("unblock.db");
    let target = unblock.join("issues.jsonl");

    let bin = env!("CARGO_BIN_EXE_export_kill");
    let mut child = Command::new(bin)
        .arg(&db_path)
        .arg(&target)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the export_kill helper");

    // Wait for the READY-EXPORTED marker: a COMPLETE target now exists and the loop is exporting.
    let stdout = child.stdout.take().expect("child stdout");
    let mut reader = std::io::BufReader::new(stdout);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut saw_marker = false;
    let mut line = String::new();
    while Instant::now() < deadline {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .expect("read the helper's stdout");
        assert!(n > 0, "the helper's stdout closed before READY-EXPORTED");
        if line.trim_end() == "READY-EXPORTED" {
            saw_marker = true;
            break;
        }
    }
    assert!(
        saw_marker,
        "timed out waiting for the READY-EXPORTED marker"
    );

    // Let a few more exports run, then SIGKILL mid-write (uncatchable, no destructors).
    std::thread::sleep(Duration::from_millis(50));
    child.kill().expect("SIGKILL the helper");
    let status = child.wait().expect("wait for the killed helper");
    assert!(!status.success(), "a SIGKILLed process never exits 0");

    // The target is ALWAYS a COMPLETE JSONL (the atomic rename is all-or-nothing): every line parses
    // and the count is the full seeded corpus, never a torn partial write.
    let content = std::fs::read_to_string(&target).expect("read the target after the kill");
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(
        lines.len(),
        KILL_SEED,
        "the target holds a complete export, never a torn partial write"
    );
    for l in &lines {
        let value: serde_json::Value =
            serde_json::from_str(l).expect("every line is a complete, valid JSON record");
        assert!(
            value.get("id").is_some(),
            "each line is a full issue record with an id"
        );
    }
}
