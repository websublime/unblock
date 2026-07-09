//! T3.4/C5 test-only helper binary (NFR-4) — drives the REAL `unblock_sync::export_jsonl` in a loop
//! against a real libsql workspace so `tests/atomic_failure.rs` can `Child::kill()` (SIGKILL) it
//! mid-write and prove the atomic temp->fsync->rename keeps the target a COMPLETE file across a hard
//! kill (never a torn partial write). Reuses the T3.2 raw-libsql `[[bin]]` precedent
//! (`unblock-storage/tests/bin/c5_abandoned_tx.rs`).
//!
//! Source under `tests/bin/`, never `src/` — it never ships via `dist`; the shipped lib is unaffected.
//! Usage: `export_kill <db_path> <jsonl_target>`. It opens + migrates + seeds the db, does ONE export
//! (so a COMPLETE target exists before the parent kills it), prints the `READY-EXPORTED` marker
//! (flushed), then loops exporting until killed (time-bounded so it self-terminates if the parent
//! misses it). The parent SIGKILLs mid-loop; a fresh read of the target must be a complete valid JSONL.

#![forbid(unsafe_code)]

use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::{TimeZone, Utc};
use unblock_model::{Issue, Priority, Status};
use unblock_storage::{LibsqlStorage, Storage};
use unblock_sync::{ExportOptions, export_jsonl};

/// The number of issues seeded — enough that each export writes a non-trivial file (a wide kill
/// window). MUST match the `SEED` the parent `tests/atomic_failure.rs` asserts.
const SEED: usize = 800;

/// Self-terminate after this long if the parent never kills the helper (belt-and-braces).
const MAX_RUN: Duration = Duration::from_mins(1);

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let db_path = PathBuf::from(
        args.next()
            .expect("usage: export_kill <db_path> <jsonl_target>"),
    );
    let target = PathBuf::from(
        args.next()
            .expect("usage: export_kill <db_path> <jsonl_target>"),
    );
    let confine_root = target
        .parent()
        .expect("the jsonl target must live under a .unblock dir")
        .to_path_buf();

    let storage = LibsqlStorage::open_local(&db_path)
        .await
        .expect("open_local");
    storage.migrate().await.expect("migrate");

    // Seed a corpus so each export writes a non-trivial file (a wide mid-write kill window).
    let ts = Utc
        .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
        .single()
        .expect("ts");
    let issues: Vec<Issue> = (0..SEED)
        .map(|i| Issue {
            id: format!("ub-{i:05}"),
            title: format!("issue {i}"),
            priority: Priority::MEDIUM,
            status: Status::Open,
            created_at: ts,
            updated_at: ts,
            ..Issue::default()
        })
        .collect();
    storage
        .create_issues(&issues, "seeder")
        .await
        .expect("seed");

    let opts = ExportOptions::default();

    // ONE export first, so a COMPLETE target exists before the loop; the parent's kill then lands
    // during a SUBSEQUENT export while a complete file is already in place.
    export_jsonl(&storage, &target, &confine_root, &opts)
        .await
        .expect("first export");
    println!("READY-EXPORTED");
    std::io::stdout().flush().expect("flush the marker");

    // Loop exporting until killed (time-bounded self-termination).
    let deadline = Instant::now() + MAX_RUN;
    while Instant::now() < deadline {
        // Errors are ignored: the parent SIGKILLs mid-write; the loop's job is only to keep the atomic
        // write path busy so the kill lands inside an export.
        let _ = export_jsonl(&storage, &target, &confine_root, &opts).await;
    }
}
