//! T3.2/C5 — SIGKILL abandoned-tx WAL-recovery rollback (FR-17/NFR-5), the DETERMINISTIC anchor for
//! AC clause (a) "SIGTERM mid-write leaves no WAL corruption" over the CRASH-recovery path (no real
//! signal handler involved — the hard-exit / no-destructors semantics of a SIGKILL are the exact
//! semantics the `shutdown.rs` second-signal `process::exit` escalation relies on, spine §4.2).
//!
//! Spawns the `c5_abandoned_tx` helper `[[bin]]` (test-only, `tests/bin/`, never `src/`/shipped) —
//! it drives RAW libsql to hold an UNCOMMITTED `BEGIN IMMEDIATE` tx open against a PRE-MIGRATED
//! workspace db, printing a `READY-IN-TX` marker once the tx genuinely holds uncommitted rows (some
//! of which have spilled to the `-wal` sidecar, per the helper's small `cache_size`). Once the marker
//! is observed the parent `Child::kill()`s it (SIGKILL — uncatchable, no destructors run; unsafe-free,
//! no libc). A fresh reopen (via the PUBLIC `LibsqlStorage::open_local`, which runs `SQLite`'s
//! ordinary WAL-recovery open — never a raw file read) must see a clean `integrity_check()` AND
//! `count == 0`: the parent's own `migrate()` is a SEPARATELY committed tx, so a stray committed row
//! from the abandoned tx would show up as `count >= 1`, making `count == 0` genuinely non-vacuous.
#![cfg(unix)]

use std::io::BufRead;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use unblock_model::ListFilters;
use unblock_storage::{LibsqlStorage, Storage};

#[tokio::test]
async fn sigkill_mid_tx_leaves_zero_rows_and_a_clean_integrity_check() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("unblock.db");

    // Pre-migrate via the PUBLIC open path — a SEPARATELY committed tx, so a stray committed row
    // from the helper's abandoned tx would make count == 1, giving count == 0 real meaning below.
    {
        let storage =
            LibsqlStorage::open_local(&db_path, unblock_storage::DEFAULT_WRITE_LOCK_TIMEOUT_MS)
                .await
                .expect("open_local");
        storage.migrate().await.expect("migrate");
        // `storage` drops here: the parent's own connections close before the helper opens the SAME
        // file, so there is no cross-connection contention on the open.
    }

    let bin = env!("CARGO_BIN_EXE_c5_abandoned_tx");
    let mut child = Command::new(bin)
        .arg(&db_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the c5_abandoned_tx helper");

    // Wait for the READY-IN-TX marker: the tx is genuinely open with uncommitted rows at this point.
    let stdout = child.stdout.take().expect("child stdout");
    let mut reader = std::io::BufReader::new(stdout);
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut saw_marker = false;
    let mut line = String::new();
    while Instant::now() < deadline {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .expect("read the helper's stdout");
        assert!(
            n > 0,
            "the helper's stdout closed before the READY-IN-TX marker"
        );
        if line.trim_end() == "READY-IN-TX" {
            saw_marker = true;
            break;
        }
    }
    assert!(saw_marker, "timed out waiting for the READY-IN-TX marker");

    // SIGKILL — uncatchable, no destructors: the abandoned tx is never committed, never rolled back
    // cooperatively; only `SQLite`'s own crash-recovery on the next open discards it.
    child.kill().expect("SIGKILL the helper");
    let status = child.wait().expect("wait for the killed helper");
    assert!(!status.success(), "a SIGKILLed process never exits 0");

    // A fresh reopen via the SAME public open path (runs SQLite's ordinary WAL-recovery open).
    let reopened =
        LibsqlStorage::open_local(&db_path, unblock_storage::DEFAULT_WRITE_LOCK_TIMEOUT_MS)
            .await
            .expect("reopen after SIGKILL");
    let problems = reopened.integrity_check().await.expect("integrity_check");
    assert!(
        problems.is_empty(),
        "WAL recovery must leave a clean database, got: {problems:?}"
    );

    let filters = ListFilters {
        include_closed: true,
        include_deferred: true,
        ..ListFilters::default()
    };
    let count = reopened.list_issues(&filters).await.expect("list").len();
    assert_eq!(
        count, 0,
        "the abandoned tx's 200 uncommitted rows must be discarded entirely (zero, never partial)"
    );
}
