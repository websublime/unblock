//! File-backed heavy-corpus parallel stress (T3.5.1 Verify follow-up).
//!
//! **Un-gated** (unlike `tests/scale.rs`, which needs `--features testkit` for its
//! `testkit::seed_corpus` import) so this runs in the always-on `cargo test --workspace` set. Being
//! un-gated, it builds its own heavy batch inline rather than importing the testkit-gated
//! `seed_corpus`.
//!
//! This is the **file analogue of `open_in_memory_parallel_first_write_stress`** (`src/libsql/mod.rs`)
//! with a *heavy* bulk workload: [`TASKS`] parallel tasks each open their own
//! [`LibsqlStorage::open_local`]-backed store, migrate it, insert one [`HEAVY_ROWS`]-issue batch via a
//! single [`Storage::create_issues`] call, read every row back and drop the store. Every task must
//! succeed — zero failures.
//!
//! T3.5.1 wrote this test to pin a boundary: heavy work on `open_local`, never on `open_in_memory`,
//! on the theory that the in-memory flake was a shared-cache registry race the file path could not
//! hit. ub-q1u disproved that theory. The flake was libsql closing every dropped connection handle
//! twice, and the file path was exposed in exactly the same way. The fix is
//! `src/libsql/close_once.rs`. No boundary between the two constructors remains, so this test now
//! stands on its own as a parallel heavy-batch stress of the file path.

use chrono::{DateTime, TimeZone, Utc};

use unblock_model::Issue;
use unblock_storage::{DEFAULT_WRITE_LOCK_TIMEOUT_MS, LibsqlStorage, Storage, StorageError};

/// The T3.5.1 heavy-batch row count — the size of the single `create_issues` batch per task.
const HEAVY_ROWS: usize = 902;

/// Parallel tasks, each driving its own file-backed store through one [`HEAVY_ROWS`]-issue batch — the
/// file analogue of `open_in_memory_parallel_first_write_stress`'s 32 tasks (halved here since every
/// task does a heavy bulk insert rather than a single row, keeping total runtime bounded).
const TASKS: usize = 16;

fn ts(year: i32, month: u32, day: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, 0, 0, 0).unwrap()
}

/// Build the `i`-th synthetic issue for `task`'s heavy batch: a unique `ub-h<task>-<i>` id (zero-padded
/// so every id is syntactically valid) at a fixed epoch. Mirrors
/// `open_in_memory_parallel_first_write_stress`'s `Issue { ..Issue::default() }` build (`src/libsql/mod.rs`)
/// and `testkit::seed_corpus`'s `seed_issue` shape (`src/testkit.rs`).
fn heavy_issue(task: usize, i: usize, created: DateTime<Utc>) -> Issue {
    Issue {
        id: format!("ub-h{task}-{i:04}"),
        title: format!("heavy stress task {task} issue {i}"),
        created_at: created,
        updated_at: created,
        ..Issue::default()
    }
}

/// The file-backed heavy-corpus analogue of `open_in_memory_parallel_first_write_stress`: [`TASKS`]
/// parallel tasks each `open_local` their own tempdir-backed store, migrate it, insert one
/// [`HEAVY_ROWS`]-issue batch via a single `create_issues` call, then read every row back. Every task
/// must succeed.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn file_backed_heavy_corpus_parallel_stress() {
    let created = ts(2026, 1, 1);
    let mut handles = Vec::new();
    for task in 0..TASKS {
        handles.push(tokio::spawn(async move {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("unblock.db");
            let storage = LibsqlStorage::open_local(&path, DEFAULT_WRITE_LOCK_TIMEOUT_MS).await?;
            storage.migrate().await?;

            let issues: Vec<Issue> = (0..HEAVY_ROWS)
                .map(|i| heavy_issue(task, i, created))
                .collect();
            storage.create_issues(&issues, "heavy-stress").await?;

            // Read every inserted row back through the public API — the non-vacuous proof the whole
            // heavy batch landed (not merely that `create_issues` returned `Ok`).
            let ids: Vec<String> = issues.iter().map(|issue| issue.id.clone()).collect();
            let fetched = storage.get_issues(&ids).await?;
            assert_eq!(
                fetched.len(),
                HEAVY_ROWS,
                "task {task}: every row in the heavy batch must land"
            );

            drop(storage);
            drop(dir);
            Ok::<(), StorageError>(())
        }));
    }

    for (task, handle) in handles.into_iter().enumerate() {
        handle
            .await
            .expect("join")
            .unwrap_or_else(|e| panic!("task {task} failed: {e:?}"));
    }
}
